//! Common utilities for bridging Minecraft chat to arbitrary chat platforms.

use std::{
    collections::VecDeque,
    ops::{Deref, DerefMut},
    time::Instant,
};

use azalea::{
    chat::ChatPacket,
    ecs::{
        app::{App, Plugin},
        event::{EventReader, EventWriter},
        system::Query,
    },
    entity::Local,
    GameProfileComponent,
};
use bevy_ecs::{
    entity::Entity,
    query::With,
    schedule::IntoSystemDescriptor,
    system::{ResMut, Resource},
};

use crate::azalea_avoid_chat_kick;

pub struct BridgePlugin<T: Clone + Sync + Send + 'static>(std::marker::PhantomData<T>);
impl<T: Clone + Sync + Send + 'static> Default for BridgePlugin<T> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<T: Clone + Sync + Send + 'static> Plugin for BridgePlugin<T> {
    fn build(&self, app: &mut App) {
        app.add_event::<FromMinecraftEvent>()
            .add_event::<ToMinecraftEvent<T>>()
            .add_event::<BridgeInfoEvent<T>>()
            .init_resource::<RecentFromMinecraft>()
            .add_system(from_minecraft)
            .add_system(to_minecraft::<T>)
            .add_system(pop_no_longer_recent_messages.after(from_minecraft));
    }
}

/// We received a message from Minecraft. This is what you should show in your
/// bridge. This may not be exactly the same message shown in Minecraft, since
/// it attempts to de-duplicate messages.
pub struct FromMinecraftEvent {
    pub content: String,
    pub packet: ChatPacket,
}

/// We're sending a message to Minecraft from your bridge.
pub struct ToMinecraftEvent<T: Clone + Sync + Send + 'static> {
    pub username: String,
    pub content: String,
    pub context: T,
}

pub struct BridgeInfoEvent<T: Clone + Sync + Send + 'static> {
    pub kind: BridgeInfoKind,
    pub context: T,
}
pub enum BridgeInfoKind {
    Ack,
    NotInServer,
    IllegalMessage,
}

#[derive(Clone)]
pub struct RecentMessage {
    pub content: String,
    pub packet: ChatPacket,
    /// The number of times the message was sent. 1 if the message was sent once.
    pub sent_count: usize,
    pub sent_at: Instant,
}
#[derive(Resource, Default)]
pub struct RecentFromMinecraft(VecDeque<RecentMessage>);
impl Deref for RecentFromMinecraft {
    type Target = VecDeque<RecentMessage>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for RecentFromMinecraft {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

fn from_minecraft(
    mut recent_from_minecraft: ResMut<RecentFromMinecraft>,
    mut events: EventReader<azalea::chat::ChatReceivedEvent>,
    mut from_minecraft_events: EventWriter<FromMinecraftEvent>,
    query: Query<&GameProfileComponent, With<Local>>,
) {
    for event in events.iter() {
        println!(
            "Got Minecraft chat packet: {}",
            event.packet.message().to_ansi()
        );

        let game_profile = query.single();
        if event.packet.username() == Some(game_profile.name.clone()) {
            // we sent this message lol
            return;
        }

        let message_string = event.packet.message().to_string();

        // check if the message is the same as one of the recent messages
        for (i, recent_message) in recent_from_minecraft.clone().iter().enumerate() {
            if recent_message.content == message_string {
                // remove it and add it back with the sent_count increased
                recent_from_minecraft.remove(i);
                let new_sent_count = recent_message.sent_count + 1;
                recent_from_minecraft.push_back(RecentMessage {
                    content: message_string.clone(),
                    sent_count: new_sent_count,
                    sent_at: Instant::now(),
                    packet: event.packet.clone(),
                });

                // if it's a power of 2, send it to discord with [x<number>] at the end
                if new_sent_count.is_power_of_two() {
                    from_minecraft_events.send(FromMinecraftEvent {
                        content: format_for_repeats(&message_string, new_sent_count),
                        packet: event.packet.clone(),
                    });
                }
                return;
            }
        }
        recent_from_minecraft.push_back(RecentMessage {
            content: message_string.clone(),
            sent_count: 1,
            sent_at: Instant::now(),
            packet: event.packet.clone(),
        });
        from_minecraft_events.send(FromMinecraftEvent {
            content: message_string,
            packet: event.packet.clone(),
        });
    }
}

fn to_minecraft<T: Clone + Sync + Send + 'static>(
    query: Query<Entity, With<Local>>,
    mut events: EventReader<ToMinecraftEvent<T>>,
    mut send_chat_events: EventWriter<azalea_avoid_chat_kick::SendChatEvent>,
    mut bridge_error_events: EventWriter<BridgeInfoEvent<T>>,
) {
    for event in events.iter() {
        let Ok(entity) =
            query.get_single() else {
                // the bot isn't on the server
                bridge_error_events.send(BridgeInfoEvent {
                    context: event.context.clone(),
					kind: BridgeInfoKind::NotInServer,
                });
                return;
            };

        // check if a message is legal and add it to the queue!
        let message_content = format!("/me <{}> {}", event.username, event.content);

        let chat_message_event =
            azalea_avoid_chat_kick::SendChatEvent::new(entity, &message_content);

        if chat_message_event.is_none() {
            bridge_error_events.send(BridgeInfoEvent {
                context: event.context.clone(),
                kind: BridgeInfoKind::IllegalMessage,
            });
            println!("illegal message");
            return;
        }
        let chat_message_event = chat_message_event.unwrap();

        bridge_error_events.send(BridgeInfoEvent {
            context: event.context.clone(),
            kind: BridgeInfoKind::Ack,
        });
        send_chat_events.send(chat_message_event);
        println!("send chat event");
    }
}

fn pop_no_longer_recent_messages(
    mut recent_from_minecraft: ResMut<RecentFromMinecraft>,
    mut from_minecraft_events: EventWriter<FromMinecraftEvent>,
) {
    loop {
        let front_message = {
            // if there's at most 5 messages and the oldest message is less than max_wait_time seconds old, we're good

            let waited_enough = recent_from_minecraft
                .front()
                .map(|m| {
                    // we're more lenient with the waiting if they sent a lot of messages
                    let max_wait_time = if m.sent_count > 32 {
                        16
                    } else if m.sent_count > 16 {
                        8
                    } else {
                        2
                    };
                    m.sent_at.elapsed().as_secs() > max_wait_time
                })
                .unwrap_or(false);
            if recent_from_minecraft.len() <= 5 && !waited_enough {
                break;
            }
            recent_from_minecraft.pop_front().expect(
                "we just checked to make sure there's stuff in recent_items so it shouldn't be empty",
            )
        };
        // if it's a power of 2 that means we already sent it
        if front_message.sent_count > 2 && !front_message.sent_count.is_power_of_two() {
            from_minecraft_events.send(FromMinecraftEvent {
                content: format_for_repeats(&front_message.content, front_message.sent_count),
                packet: front_message.packet,
            });
        }
    }
}

fn format_for_repeats(message: &str, sent_count: usize) -> String {
    if sent_count == 1 {
        return message.to_string();
    }
    format!("{message} [x{sent_count}]")
}
