use chrono::Utc;
use jsonwebtoken::Header;
use log::info;
use mongodb::bson::doc;
use rand::{thread_rng, Rng};
use rocket::{serde::json::Json, State};
use serde::{Deserialize, Serialize};
use typescript_type_def::TypeDef;

use crate::rest_controller::{
    auth::{JwtClaims, JwtTokenError, User},
    JwtEncodingKey, MongoDatabase, UserId,
};

#[derive(Debug, Serialize, TypeDef)]
#[serde(tag = "status")]
pub enum ResponseAuthenticate {
    /// For unregistered users Success will alway be returned.
    Success {
        user_id: String,
        user_registered: bool,
        refreshed_token: String,
    },

    /// Will only be returned for registered users.
    TokenExpired {},

    /// An internal error occurred.
    /// The user should try again to create a session.
    InternalError {
        message: String,
    },

    InvalidUser {},

    /// We don't have any user authentication.
    NoAuthentication {},

    /// The target user now requires login.
    UserRequiresLogin {},
}

// FIXME: Move user management into an own module!
async fn register_new_user(database: &MongoDatabase) -> anyhow::Result<JwtClaims> {
    let users = database.0.collection::<User>("users");
    let user = User {
        // Never allow the msb to be set.
        user_id: thread_rng().gen::<UserId>() >> 1,
        registration: None,
    };

    info!("Registering new user {}.", user.user_id);
    // FIXME: Loop this until we found a unique user id
    users.insert_one(&user, None).await?;

    Ok(JwtClaims {
        exp: 0,
        iat: 0,

        user_id: user.user_id.to_string(),
        user_registered: false,
    })
}

#[rocket::post("/authenticate", data = "<payload>")]
pub async fn post_authenticate(
    database: &State<MongoDatabase>,
    current_token: Result<JwtClaims, JwtTokenError>,
    jwt_encoding_key: &State<JwtEncodingKey>,
    payload: Json<PayloadAuthenticate>,
) -> Json<ResponseAuthenticate> {
    let current_token = match current_token {
        Ok(claims) => Some((claims, false)),
        Err(JwtTokenError::Expired { claims }) => Some((claims, true)),
        _ => None, // No token given or the given token is not parseable/verifyable.
    };

    let mut claims = {
        let users = database.0.collection::<User>("users");
        if let Some((mut claims, expired)) = current_token {
            let mut token_expires = chrono::Duration::days(1);
            match users
                .find_one(doc! { "user_id": claims.user_id.to_string() }, None)
                .await
            {
                Ok(Some(user)) => {
                    if user.registration.is_some() {
                        if !claims.user_registered {
                            /* You must login for that user. */
                            return Json(ResponseAuthenticate::UserRequiresLogin {});
                        }

                        if expired {
                            // TODO: May just issue a new token if the last login wasn't XY months ago.
                        }

                        if expired {
                            return Json(ResponseAuthenticate::TokenExpired {});
                        }

                        token_expires = chrono::Duration::days(7);
                    } else {
                        // User has no registration.
                        // Everybody with the token can access the user.
                        token_expires = chrono::Duration::days(356);
                    }
                }
                Ok(None) => {
                    if !payload.create_account {
                        return Json(ResponseAuthenticate::InvalidUser {});
                    }
                    // FIXME: Create a new user.
                }
                Err(error) => {
                    return Json(ResponseAuthenticate::InternalError {
                        message: format!("{}", error),
                    })
                }
            }

            claims.exp = (Utc::now().naive_utc() + token_expires).timestamp() as u64;
            claims
        } else if let Some((_username, _password)) = &payload.credentials {
            // FIXME: TODO!
            return Json(ResponseAuthenticate::InternalError {
                message: "login not yet supported".to_string(),
            });
        } else if payload.create_account {
            match register_new_user(&*database).await {
                Ok(mut claims) => {
                    claims.exp =
                        (Utc::now().naive_utc() + chrono::Duration::days(356)).timestamp() as u64;
                    claims
                }
                Err(error) => {
                    return Json(ResponseAuthenticate::InternalError {
                        message: format!("{}", error),
                    })
                }
            }
        } else {
            return Json(ResponseAuthenticate::NoAuthentication {});
        }
    };

    claims.iat = Utc::now().naive_utc().timestamp() as u64;
    let token = jsonwebtoken::encode(&Header::default(), &claims, &jwt_encoding_key.0);
    let token = match token {
        Ok(token) => token,
        Err(error) => {
            return Json(ResponseAuthenticate::InternalError {
                message: format!("failed to generate token: {}", error),
            })
        }
    };

    Json(ResponseAuthenticate::Success {
        user_id: claims.user_id,
        user_registered: claims.user_registered,
        refreshed_token: token,
    })
}

#[derive(Deserialize, TypeDef)]
pub struct PayloadAuthenticate {
    #[serde(default)]
    credentials: Option<(String, String)>,
    create_account: bool,
}
