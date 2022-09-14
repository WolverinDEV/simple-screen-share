use jsonwebtoken::{errors::ErrorKind, Validation};
use rocket::{
    async_trait,
    http::Status,
    request::{FromRequest, Outcome},
};
use serde::{Deserialize, Serialize};
use typescript_type_def::TypeDef;

use super::JwtDecodingKey;

#[derive(Debug, Serialize)]
pub enum JwtTokenError {
    MissingToken,
    /// We're not authorizing via JWT token.
    InvalidAuthorization,
    TokenInvalid,
    TokenExpired,
    ValidationFailure {
        reason: String,
    },
    Expired {
        claims: JwtClaims,
    },
}

#[async_trait]
impl<'a> FromRequest<'a> for JwtClaims {
    type Error = JwtTokenError;

    async fn from_request(request: &'a rocket::Request<'_>) -> Outcome<JwtClaims, Self::Error> {
        let token = match request.headers().get_one("authorization") {
            Some(token) => token,
            None => return Outcome::Failure((Status::BadRequest, JwtTokenError::MissingToken)),
        };

        let mut parts = token.split(' ');
        let user = match parts.next() {
            Some(user) => user,
            None => return Outcome::Failure((Status::BadRequest, JwtTokenError::TokenInvalid)),
        };
        if user != "Bearer" {
            return Outcome::Failure((Status::BadRequest, JwtTokenError::InvalidAuthorization));
        }

        let token = match parts.next() {
            Some(token) => token,
            None => return Outcome::Failure((Status::BadRequest, JwtTokenError::TokenInvalid)),
        };

        // FIXME: Validate the correctness of the user id!
        let decoding_key = request
            .rocket()
            .state::<JwtDecodingKey>()
            .expect("missing jwt decoding key");
        match jsonwebtoken::decode::<JwtClaims>(token, &decoding_key.0, &Validation::default()) {
            Ok(payload) => Outcome::Success(payload.claims),
            Err(error) => match error.kind() {
                ErrorKind::ExpiredSignature => {
                    let mut validation = Validation::default();
                    validation.validate_exp = false;
                    validation.validate_nbf = false;
                    if let Ok(payload) =
                        jsonwebtoken::decode::<JwtClaims>(token, &decoding_key.0, &validation)
                    {
                        Outcome::Success(payload.claims)
                    } else {
                        Outcome::Failure((
                            Status::Unauthorized,
                            JwtTokenError::ValidationFailure {
                                reason: format!("{}", error),
                            },
                        ))
                    }
                }
                _ => Outcome::Failure((
                    Status::Unauthorized,
                    JwtTokenError::ValidationFailure {
                        reason: format!("{}", error),
                    },
                )),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize, TypeDef)]
pub struct JwtClaims {
    pub user_id: String,
    pub user_registered: bool,

    /// Will be zero for unregistered users.
    pub exp: u64,
    pub iat: u64,
}

impl JwtClaims {
    pub fn user_id(&self) -> Option<u64> {
        self.user_id.parse::<u64>().ok()
    }
}

/// An user id which has been registered within the database.
/// Note: The msb will never be set!
pub type UserId = u64;

// FIXME: Move user management into an own module!
#[derive(Debug, Deserialize, Serialize)]
pub struct User {
    pub user_id: UserId,
    pub registration: Option<UserRegistration>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UserRegistration {
    pub username: String,
    pub password: String, // TODO: How to do this?
}
