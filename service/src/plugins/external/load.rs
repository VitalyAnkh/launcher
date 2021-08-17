use crate::PluginConfig;

use futures_lite::{future::zip, Stream, StreamExt};
use postage::mpsc::Sender;
use postage::prelude::Stream as PostageStream;
use postage::prelude::*;
use regex::Regex;
use std::path::{Path, PathBuf};

/// Fetches plugins installed on the system in parallel.
///
/// Searches plugin paths from highest to least priority. User plugins will override
/// distribution plugins. Plugins are loaded in the order they are found.
pub async fn from_paths(mut tx: Sender<(PathBuf, PluginConfig, Option<Regex>)>) {
    let (mut tasks_tx, mut tasks_rx) = postage::mpsc::channel(8);

    // Spawns a background task to run in parallel for each plugin found
    let task_spawner = async move {
        for path in crate::plugin_paths() {
            let loadable_plugins = from_path(&path);
            futures_lite::pin!(loadable_plugins);

            while let Some((source, config)) = loadable_plugins.next().await {
                let future = smol::unblock(move || crate::plugins::config::load(&source, &config));
                if tasks_tx.send(smol::spawn(future)).await.is_err() {
                    break;
                }
            }
        }
    };

    // This future ensures that plugins are returned in the order they were spawned.
    let task_listener = async move {
        while let Some(task) = tasks_rx.recv().await {
            if let Some(plugin) = task.await {
                if tx.send(plugin).await.is_err() {
                    break;
                }
            }
        }
    };

    zip(task_spawner, task_listener).await;
}

/// Loads all plugin information found in the given path.
pub fn from_path(path: &Path) -> impl Stream<Item = (PathBuf, PathBuf)> + '_ {
    gen_z::gen_z(move |mut z| async move {
        if let Ok(readdir) = path.read_dir() {
            for entry in readdir.filter_map(Result::ok) {
                let source = entry.path();
                if !source.is_dir() {
                    continue;
                }

                let config = source.join("plugin.ron");
                if !config.exists() {
                    continue;
                }

                z.send((source, config)).await;
            }
        }
    })
}
