use tracing::info;

use serenity::prelude::*;
use serenity::async_trait;
use serenity::client::{Client, Context, EventHandler};
use serenity::client::bridge::gateway::{ShardId, ShardManager};
use serenity::http::Http;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::framework::standard::{
    StandardFramework,
    CommandResult,
    Args,
    macros::{
        command,
        group
    }
};
use tokio::sync::Mutex;

use lavalink_rs::{gateway::*, model::*, LavalinkClient};
use songbird::SerenityInit;

use std::env;
use std::sync::Arc;

struct Lavalink;
impl TypeMapKey for Lavalink {
    type Value = LavalinkClient;
}

struct ShardManagerContainer;

impl TypeMapKey for ShardManagerContainer {
    type Value = Arc<Mutex<ShardManager>>;
}

struct Handler;
struct LavalinkHandler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);
    }
}

#[async_trait]
impl LavalinkEventHandler for LavalinkHandler {
    async fn track_start(&self, _client: LavalinkClient, event: TrackStart) {
        info!("Track started!\nGuild: {}", event.guild_id);
    }
    async fn track_finish(&self, _client: LavalinkClient, event: TrackFinish) {
        info!("Track finished!\nGuild: {}", event.guild_id);
    }
}

#[group]
#[commands(ping)]
struct General;

#[tokio::main]
async fn main() {
    let framework = StandardFramework::new().configure(|c| c.prefix("!")).group(&GENERAL_GROUP);

    let token = env::var("DISCORD_TOKEN").expect("token");

    let http = Http::new_with_token(&token);

    let bot_id = match http.get_current_application_info().await {
        Ok(info) => info.id,
        Err(why) => panic!("Could not access application info: {:?}", why),
    };


    let mut client = Client::builder(&token)
        .event_handler(Handler)
        .framework(framework)
        .register_songbird()
        .await
        .expect("Err creating client");


    let lava_client = LavalinkClient::builder(bot_id)
        .set_host("127.0.0.1:2333")
        .set_password(
            env::var("LAVALINK_PASSWORD").unwrap_or_else(|_| "youshallnotpass".to_string()),
        )
        .build(LavalinkHandler)
        .await.unwrap();


    {
        let mut data = client.data.write().await;
        data.insert::<ShardManagerContainer>(Arc::clone(&client.shard_manager));
        data.insert::<Lavalink>(lava_client);
    }

    if let Err(why) = client.start().await {
        println!("An error occurred while running the client: {:?}", why);
    }
}

#[command]
async fn join(ctx: &Context, msg: &Message) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;

    let channel_id = guild.voice_states.get(&msg.author.id).and_then(|voice_state| voice_state.channel_id);
    let connect_to = match channel_id {
        Some(channel) => channel,
        None => {
            msg.reply(ctx, "Join a voice channel first.").await?;

            return Ok(());
        }
    };

    let manager = songbird::get(ctx).await.unwrap().clone();

    let (_, handler) = manager.join_gateway(guild_id, connect_to).await;

    match handler {
        Ok(connection_info) => {
            let data = ctx.data.read().await;
            let lava_client = data.get::<Lavalink>().unwrap().clone();
            lava_client.create_session_with_songbird(&connection_info).await?;

            msg.channel_id.say(ctx, &format!("Joined {}", connect_to.mention())).await?;
        },
        Err(_) => {
            msg.channel_id.say(ctx, &format!("Error joining {}", connect_to.mention())).await?;
        }
    }

    Ok(())
}

#[command]
async fn leave(ctx: &Context, msg: &Message) -> CommandResult {
    let guild = msg.guild(&ctx.cache).await.unwrap();
    let guild_id = guild.id;

    let manager = songbird::get(ctx).await.unwrap().clone();
    let has_handler = manager.get(guild_id).is_some();

    if has_handler {
        if let Err(e) = manager.remove(guild_id).await {
            msg.channel_id
                .say(&ctx.http, format!("Failed: {:?}", e))
                .await?;
        }

        {
            let data = ctx.data.read().await;
            let lava_client = data.get::<Lavalink>().unwrap().clone();
            lava_client.destroy(guild_id).await?;
        }

        msg.channel_id.say(&ctx.http, "Left voice channel").await?;
    } else {
        msg.reply(&ctx.http, "Not in a voice channel").await?;
    }

    Ok(())

}

#[command]
#[min_args(1)]
async fn play(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    let query = args.message().to_string();

    let guild_id = match ctx.cache.guild_channel(msg.channel_id).await {
        Some(channel) => channel.guild_id,
        None => {
            msg.channel_id
                .say(&ctx.http, "Error finding channel info")
                .await?;

            return Ok(());
        }
    };

    let lava_client = {
        let data = ctx.data.read().await;
        data.get::<Lavalink>().unwrap().clone()
    };

    let manager = songbird::get(ctx).await.unwrap().clone();

    if let Some(_handler) = manager.get(guild_id) {

        let query_information = lava_client.auto_search_tracks(&query).await?;

        if query_information.tracks.is_empty() {
            msg.channel_id
                .say(&ctx, "Could not find any video of the search query.")
                .await?;
            return Ok(());
        }

        if let Err(why) = &lava_client
            .play(guild_id, query_information.tracks[0].clone())
            .queue()
            .await
        {
            eprintln!("{}", why);
            return Ok(());
        };
        msg.channel_id
            .say(
                &ctx.http,
                format!(
                    "Added to queue: {}",
                    query_information.tracks[0].info.as_ref().unwrap().title
                ),
            )
            .await?;
    } else {
        msg.channel_id
            .say(
                &ctx.http,
                "Use `!join` first, to connect the bot to your current voice channel.",
            )
            .await?;
    }

    Ok(())
}

#[command]
#[aliases(np)]
async fn now_playing(ctx: &Context, msg: &Message) -> CommandResult {
    let data = ctx.data.read().await;
    let lava_client = data.get::<Lavalink>().unwrap().clone();

    if let Some(node) = lava_client.nodes().await.get(&msg.guild_id.unwrap().0) {
        if let Some(track) = &node.now_playing {
            msg.channel_id
                .say(
                    &ctx.http,
                    format!("Now Playing: {}", track.track.info.as_ref().unwrap().title),
                )
                .await?;
        } else {
            msg.channel_id
                .say(&ctx.http, "Nothing is playing at the moment.")
                .await?;
        }
    } else {
        msg.channel_id
            .say(&ctx.http, "Nothing is playing at the moment.")
            .await?;
    }

    Ok(())
}

#[command]
async fn skip(ctx: &Context, msg: &Message) -> CommandResult {
    let data = ctx.data.read().await;
    let lava_client = data.get::<Lavalink>().unwrap().clone();

    if let Some(track) = lava_client.skip(msg.guild_id.unwrap()).await {
        msg.channel_id
            .say(
                ctx,
                format!("Skipped: {}", track.track.info.as_ref().unwrap().title),
            )
            .await?;
    } else {
        msg.channel_id.say(&ctx.http, "Nothing to skip.").await?;
    }

    Ok(())
}



#[command]
async fn ping(ctx: &Context, msg: &Message) -> CommandResult {
    let data = ctx.data.read().await;

    let shard_manager = match data.get::<ShardManagerContainer>() {
        Some(v) => v,
        None => {
            msg.reply(ctx, "There was a problem getting the shard manager").await?;

            return Ok(());
        }
    };

    let manager = shard_manager.lock().await;
    let runners = manager.runners.lock().await;

    let runner = match runners.get(&ShardId(ctx.shard_id)) {
        Some(runner) => runner,
        None => {
            msg.reply(ctx, "No shard found").await?;

            return Ok(());
        },
    };

    msg.reply(ctx, &format!("Ping took {:?} ms", runner.latency.unwrap().as_millis())).await?;
    Ok(())
}