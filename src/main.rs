#![feature(async_closure)]
#![feature(trivial_bounds)]

use azalea::entity::Position;
use azalea::pathfinder::BlockPosGoal;
use azalea::{Account, BlockPos, GameProfileComponent};

mod azalea_avoid_chat_kick;
mod azalea_bridge;
mod azalea_discord_bridge;
mod bevy_discord;
// mod bevy_matrix;

use azalea::brigadier::builder::literal_argument_builder::literal;
use azalea::brigadier::command_dispatcher::CommandDispatcher;
use azalea::brigadier::context::CommandContext;
use azalea::chat::ChatPacket;
use azalea::ecs::prelude::*;
use azalea::entity::metadata::Player;
use azalea::prelude::*;
use azalea::swarm::prelude::*;
use parking_lot::Mutex;
use std::env;
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

    commands.register(
        literal("ping").executes(|ctx: &CommandContext<Mutex<CommandSource>>| {
            let source = ctx.source.lock();
            source.reply("pong!");
            1
        }),
    );

    commands.register(literal("whereami").executes(
        |ctx: &CommandContext<Mutex<CommandSource>>| {
            let mut source = ctx.source.lock();
            let Some(entity) = source.entity() else {
                source.reply("You aren't in render distance!");
                return 0;
            };
            let position = source.bot.entity_component::<Position>(entity);
            source.reply(&format!(
                "You are at {}, {}, {}",
                position.x, position.y, position.z
            ));
            1
        },
    ));

    commands.register(
        literal("goto").executes(|ctx: &CommandContext<Mutex<CommandSource>>| {
            let mut source = ctx.source.lock();
            let Some(entity) = source.entity() else {
                source.reply("You aren't in render distance!");
                return 0;
            };
            let position = source.bot.entity_component::<Position>(entity);
            source
                .bot
                .goto(BlockPosGoal::from(BlockPos::from(position)));
            1
        }),
    );

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
            .add_account_with_state(
                account.clone(),
                State {
                    commands: commands.clone(),
                },
            )
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

pub struct CommandSource {
    pub bot: Client,
    pub chat: ChatPacket,
}

impl CommandSource {
    pub fn reply(&self, message: &str) {
        if self.chat.is_whisper() {
            self.bot
                .chat(&format!("/w {} {}", self.chat.username().unwrap(), message));
        } else {
            self.bot.chat(message);
        }
    }

    pub fn entity(&mut self) -> Option<Entity> {
        let username = self.chat.username()?;
        self.bot
            .entity_by::<With<Player>, (&GameProfileComponent,)>(
                |profile: &&GameProfileComponent| profile.name == username,
            )
    }
}

#[derive(Component, Default, Clone)]
pub struct State {
    pub commands: Arc<CommandDispatcher<Mutex<CommandSource>>>,
}

#[derive(Resource, Default, Clone)]
struct SwarmState;

async fn handle(bot: Client, event: azalea::Event, state: State) -> anyhow::Result<()> {
    match event {
        azalea::Event::Login => {}
        azalea::Event::Chat(chat) => {
            println!("{}", chat.message().to_ansi());
            if let (Some(username), content) = chat.split_sender_and_content() {
                if username != "py5" {
                    return Ok(());
                }

                println!("{:?}", chat.message());

                let _ = state.commands.execute(
                    content,
                    Mutex::new(CommandSource {
                        bot,
                        chat: chat.clone(),
                    }),
                );
            }
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
