use std::collections::VecDeque;

use azalea::{
    app::{App, CoreSchedule, IntoSystemAppConfig, Plugin},
    ecs::{
        event::{EventReader, EventWriter},
        schedule::IntoSystemConfig,
        system::{Res, ResMut},
    },
    prelude::*,
};

use crate::{
    azalea_bridge::{
        from_minecraft, pop_no_longer_recent_messages, to_minecraft, BridgeInfoEvent,
        BridgeInfoKind, BridgePlugin, FromMinecraftEvent, ToMinecraftEvent,
    },
    bevy_discord::{self, handle_create_reaction, handle_from_discord_events},
};

pub struct DiscordBridgePlugin {
    pub channel_id: u64,
}

impl Plugin for DiscordBridgePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(DiscordBridge {
            channel_id: self.channel_id,
            discord_queue: VecDeque::new(),
            discord_ratelimit: 0,
        })
        .add_plugin(BridgePlugin::<DiscordContext>::default())
        .add_systems((
            minecraft_to_discord_queue
                .after(from_minecraft)
                .after(pop_no_longer_recent_messages)
                .after(discord_to_minecraft),
            discord_to_minecraft
                .after(handle_from_discord_events)
                .after(to_minecraft::<DiscordContext>),
            handle_bridge_info_events
                .before(handle_create_reaction)
                .before(to_minecraft::<DiscordContext>),
            flush_to_discord_queue.in_schedule(CoreSchedule::FixedUpdate),
        ));
    }
}

#[derive(Clone)]
pub struct DiscordContext {
    pub channel_id: u64,
    pub message_id: u64,
}

#[derive(Resource)]
pub struct DiscordBridge {
    pub channel_id: u64,
    pub discord_queue: VecDeque<String>,
    /// The number of ticks we have to wait until the ratelimit is fully reset. Sending a message adds 20, if it's >= 100 we can't send messages.
    pub discord_ratelimit: usize,
}

fn minecraft_to_discord_queue(
    mut discord_bridge: ResMut<DiscordBridge>,
    mut events: EventReader<FromMinecraftEvent>,
) {
    for event in events.iter() {
        let content = event
            .content
            .to_string()
            .replace('*', "\\*")
            .replace('_', "\\_");

        discord_bridge.discord_queue.push_back(content);
    }
}

fn flush_to_discord_queue(
    mut discord_bridge: ResMut<DiscordBridge>,
    mut creating_message_events: EventWriter<bevy_discord::send::CreateMessage>,
) {
    if discord_bridge.discord_ratelimit > 0 {
        discord_bridge.discord_ratelimit -= 1;
    }
    if discord_bridge.discord_ratelimit >= 100 {
        // ratelimited!
        return;
    }
    let mut sending_messages = Vec::new();
    while let Some(content) = discord_bridge.discord_queue.pop_front() {
        discord_bridge.discord_ratelimit += 20;
        // 1000 instead of 2000 just to maybe avoid possible exploits
        if sending_messages.join("\n").len() + 1 + content.len() > 1000 {
            break;
        }
        sending_messages.push(content);
    }
    if !sending_messages.is_empty() {
        let content = sending_messages.join("\n");
        creating_message_events.send(bevy_discord::send::CreateMessage {
            channel_id: discord_bridge.channel_id,
            content,
        });
    }
}

fn discord_to_minecraft(
    discord_bridge: Res<DiscordBridge>,
    mut events: EventReader<bevy_discord::recv::MessageCreate>,
    mut to_minecraft_events: EventWriter<ToMinecraftEvent<DiscordContext>>,
) {
    for event in events.iter() {
        if event.author.bot && event.author.discriminator != 0 {
            return;
        }
        if event.channel_id.get() != discord_bridge.channel_id {
            return;
        }

        let display_name = if event.author.discriminator == 0 {
            event.author.name.clone()
        } else {
            format!("{}#{:0>4}", event.author.name, event.author.discriminator)
        };

        to_minecraft_events.send(ToMinecraftEvent {
            content: event.content.clone(),
            username: display_name,
            context: DiscordContext {
                channel_id: event.channel_id.get(),
                message_id: event.id.get(),
            },
        });
    }
}

fn handle_bridge_info_events(
    mut events: EventReader<BridgeInfoEvent<DiscordContext>>,
    mut react_events: EventWriter<bevy_discord::send::CreateReaction>,
) {
    for event in events.iter() {
        match event.kind {
            BridgeInfoKind::Ack => {
                react_events.send(bevy_discord::send::CreateReaction {
                    channel_id: event.context.channel_id,
                    message_id: event.context.message_id,
                    emoji: '👍',
                });
            }
            BridgeInfoKind::NotInServer => {
                react_events.send(bevy_discord::send::CreateReaction {
                    channel_id: event.context.channel_id,
                    message_id: event.context.message_id,
                    emoji: '👎',
                });
            }
            BridgeInfoKind::IllegalMessage => {
                react_events.send(bevy_discord::send::CreateReaction {
                    channel_id: event.context.channel_id,
                    message_id: event.context.message_id,
                    emoji: '🚫',
                });
            }
        }
    }
}
