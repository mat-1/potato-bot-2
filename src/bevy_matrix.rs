//! A Bevy plugin for controlling a Matrix bot.

use std::{pin::Pin, sync::mpsc};

use bevy_app::{App, Plugin};
use bevy_ecs::system::{Res, ResMut, Resource};
use bevy_tasks::{IoTaskPool, Task};
use futures_lite::{future, Stream, StreamExt};
use matrix_sdk::{config::SyncSettings, Client, deserialized_responses::SyncResponse};

pub struct MatrixPlugin {
    pub token: String,
}
impl Plugin for MatrixPlugin {
    fn build(&self, app: &mut App) {
        let mut pool = IoTaskPool::get();

        let token = self.token.clone();

        let (tx, rx) = mpsc::unbounded_channel();

        let task = pool.spawn(async move {
            let client = match Client::builder()
                .homeserver_url("https://matdoes.dev")
                .build()
                .await
            {
                Ok(client) => client,
                Err(err) => {
                    eprintln!("Couldn't make Matrix client with homeserver. {err}");
                    return None;
                }
            };
            if let Err(err) = client
                .login_token(&token)
                .initial_device_display_name("potato bot")
                .send()
                .await
            {
                eprintln!("Couldn't log into Matrix client with given token. {err}");
                return None;
            };

            if let Err(err) = client.sync_once(SyncSettings::default()).await {
                eprintln!("{err}");
                return None;
            };

            let sync_stream = Box::pin(client.sync_stream(SyncSettings::default()).await);

            while let Some(Ok(item)) = sync_stream.next().await {
                tx.send(item);
            }

            Some(MatrixClient {client, sync_stream})
        });
        app.insert_resource(Matrix {
            login_task: Some(task),
            client: None,
        })
        .add_system(check_login_task);
    }
}

pub struct MatrixClient {
    pub client: Client,
    pub sync_stream: Pin<Box<impl Stream<Item = Result<SyncResponse, matrix_sdk::Error>>>>
}

#[derive(Resource)]
pub struct Matrix {
    pub login_task: Option<Task<Option<MatrixClient>>>,
    pub client: Option<Client>,
}

fn check_login_task(mut matrix: ResMut<Matrix>) {
    if let Some(login_task) = &mut matrix.login_task {
        if let Some(client) = future::block_on(future::poll_once(login_task)) {
            matrix.login_task = None;
            matrix.client = client;
        }
    }
}
fn handle_from_matrix_events(mut matrix: ResMut<Matrix>) {
    let Some(client) = matrix.client else { return };

    // let sync_stream = client.sync_stream();
}

// impl Matrix {
//     pub fn new() {

//     }
// }
