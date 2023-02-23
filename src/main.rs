#![feature(async_closure)]

use azalea::Account;

mod azalea_avoid_chat_kick;
mod azalea_bridge;
mod azalea_discord_bridge;
mod bevy_discord;
mod bevy_matrix;

use azalea::prelude::*;
use azalea::swarm::prelude::*;
use azalea_protocol::packets::game::serverbound_client_command_packet::ServerboundClientCommandPacket;
use std::env;
use std::time::Duration;
use tokio::time::sleep;
use twilight_gateway::Intents;

use crate::azalea_avoid_chat_kick::AvoidKickPlugin;
use crate::azalea_discord_bridge::DiscordBridgePlugin;
use crate::bevy_discord::DiscordPlugin;

#[derive(Component, Default, Clone)]
struct State;
#[derive(Resource, Default, Clone)]
struct SwarmState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().expect("Failed to load .env file");
    env_logger::init();

    {
        use parking_lot::deadlock;
        use std::thread;
        use std::time::Duration;

        // Create a background thread which checks for deadlocks every 10s
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(10));
            let deadlocks = deadlock::check_deadlock();
            if deadlocks.is_empty() {
                continue;
            }

            println!("{} deadlocks detected", deadlocks.len());
            for (i, threads) in deadlocks.iter().enumerate() {
                println!("Deadlock #{i}");
                for t in threads {
                    println!("Thread Id {:#?}", t.thread_id());
                    println!("{:#?}", t.backtrace());
                }
            }
        });
    }

    let account = if let Ok(email) = env::var("EMAIL") {
        Account::microsoft(&email).await?
    } else {
        eprintln!("No EMAIL in env, defaulting to offline-mode.");
        Account::offline("potatobot")
    };

    let token = env::var("DISCORD_TOKEN").expect("Expected DISCORD_TOKEN in env");

    let channel_id: u64 = env::var("DISCORD_CHANNEL_ID").unwrap().parse().unwrap();

    loop {
        let error = SwarmBuilder::new()
            .add_plugin(AvoidKickPlugin)
            .add_plugin(DiscordPlugin {
                token: token.clone(),
                intents: Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT,
            })
            .add_plugin(DiscordBridgePlugin { channel_id })
            .set_handler(handle)
            .set_swarm_handler(swarm_handle)
            .add_account(account.clone())
            .start(
                env::var("SERVER_IP")
                    .expect("Expected SERVER_IP in env")
                    .as_str(),
            )
            .await;
        eprintln!("{error:?}");
        sleep(Duration::from_secs(4)).await;
    }
}

async fn handle(bot: Client, event: Event, _state: State) -> anyhow::Result<()> {
    match event {
        azalea::Event::Login => {}
        azalea::Event::Death(_) => {
            bot.write_packet(ServerboundClientCommandPacket {
                action: azalea_protocol::packets::game::serverbound_client_command_packet::Action::PerformRespawn,
            }.get());
        }
        _ => {}
    }

    Ok(())
}
async fn swarm_handle(
    mut swarm: Swarm,
    event: SwarmEvent,
    _state: SwarmState,
) -> anyhow::Result<()> {
    match &event {
        SwarmEvent::Disconnect(account) => {
            println!("bot got kicked! {}", account.username);
            tokio::time::sleep(Duration::from_secs(5)).await;
            swarm.add(account, State::default()).await?;
        }
        _ => {}
    }

    Ok(())
}
