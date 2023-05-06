#![feature(async_closure)]
#![feature(trivial_bounds)]

use azalea::Account;

mod azalea_avoid_chat_kick;
mod azalea_bridge;
mod azalea_discord_bridge;
mod bevy_discord;
// mod bevy_matrix;

use azalea::brigadier::builder::literal_argument_builder::literal;
use azalea::brigadier::command_dispatcher::CommandDispatcher;
use azalea::brigadier::context::CommandContext;
use azalea::prelude::*;
use azalea::protocol::packets::game::serverbound_client_command_packet::ServerboundClientCommandPacket;
use azalea::swarm::prelude::*;
use std::env;
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use twilight_gateway::Intents;

use crate::azalea_avoid_chat_kick::AvoidKickPlugin;
use crate::azalea_discord_bridge::DiscordBridgePlugin;
use crate::bevy_discord::DiscordPlugin;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();

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

    let token = env::var("DISCORD_TOKEN");
    if token.is_err() {
        eprintln!("No DISCORD_TOKEN in env, Discord bridge will not work.");
    }

    let channel_id: Option<u64> = env::var("DISCORD_CHANNEL_ID")
        .map(|id| {
            id.parse::<u64>()
                .expect("Expected DISCORD_CHANNEL_ID to be a u64")
        })
        .ok();

    let mut commands = CommandDispatcher::new();

    commands.register(literal("ping").executes(|ctx: &CommandContext<Swarm>| {
        for bot in ctx.source.deref().clone().into_iter() {
            bot.chat("pong!");
        }
        1
    }));

    let commands = Arc::new(commands);

    loop {
        let mut builder = SwarmBuilder::new().add_plugin(AvoidKickPlugin);
        if let Ok(token) = token.clone() {
            let channel_id = channel_id.expect("Expected DISCORD_CHANNEL_ID in env");
            builder = builder
                .add_plugin(DiscordPlugin {
                    token: token.clone(),
                    intents: Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT,
                })
                .add_plugin(DiscordBridgePlugin { channel_id });
        };
        let error = builder
            .set_handler(handle)
            .set_swarm_handler(swarm_handle)
            .set_swarm_state(SwarmState {
                commands: commands.clone(),
            })
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

#[derive(Component, Default, Clone)]
pub struct State;

#[derive(Resource, Default, Clone)]
struct SwarmState {
    pub commands: Arc<CommandDispatcher<Swarm>>,
}

async fn handle(bot: Client, event: Event, _state: State) -> anyhow::Result<()> {
    match event {
        azalea::Event::Login => {}
        azalea::Event::Death(_) => {
            bot.write_packet(ServerboundClientCommandPacket {
                action: azalea::protocol::packets::game::serverbound_client_command_packet::Action::PerformRespawn,
            }.get());
        }
        _ => {}
    }

    Ok(())
}
async fn swarm_handle(
    mut swarm: Swarm,
    event: SwarmEvent,
    state: SwarmState,
) -> anyhow::Result<()> {
    match &event {
        SwarmEvent::Chat(chat) => {
            if let (Some(username), content) = chat.split_sender_and_content() {
                if username != "py5" {
                    return Ok(());
                }
                let _ = state.commands.execute(content.into(), Arc::new(swarm));
            }
        }
        SwarmEvent::Disconnect(account) => {
            println!("bot got kicked! {}", account.username);
            tokio::time::sleep(Duration::from_secs(5)).await;
            swarm
                .add_with_exponential_backoff(account, State::default())
                .await;
        }
        _ => {}
    }

    Ok(())
}
