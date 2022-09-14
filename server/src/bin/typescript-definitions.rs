use simple_screen_share::{
    rest_controller::routes::{
        PayloadAuthenticate, ResponseAuthenticate, ResponseRoomCreate, ResponseRoomJoin,
    },
    rtp_server::messages::{C2SMessage, S2CMessage},
};
use typescript_type_def::{write_definition_file, DefinitionFileOptions};

type ApiTypes = (C2SMessage, S2CMessage);

type RestTypes = (
    PayloadAuthenticate,
    ResponseAuthenticate,
    ResponseRoomJoin,
    ResponseRoomCreate,
);

fn main() {
    {
        let mut options = DefinitionFileOptions::default();
        options.root_namespace = None;

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open("../client/generated/rtp-messages.ts")
            .expect("failed to open messages definition file");

        write_definition_file::<_, ApiTypes>(&mut file, options).unwrap();
    };

    {
        let mut options = DefinitionFileOptions::default();
        options.root_namespace = None;

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open("../client/generated/rest-types.ts")
            .expect("failed to open rest-types definition file");

        write_definition_file::<_, RestTypes>(&mut file, options).unwrap();
    };
}
