use notify::{event::RemoveKind, Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio_util::sync::CancellationToken;
use tracing::{info, Span};

use crate::append_path::Append;
use crate::compress::compress_epicly;

pub async fn watch(
    span: Span,
    token: CancellationToken,
    config: Config,
) -> Result<(), notify::Error> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(12);
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            tx.blocking_send(res)
                .expect("failed to send message over channel")
        },
        config,
    )?;

    watcher.watch(std::path::Path::new("static"), RecursiveMode::Recursive)?;

    while let Some(received) = tokio::select! {
            received = rx.recv() => received,
            _ = token.cancelled() => return Ok(())
    } {
        match received {
            Ok(event) => {
                if event.kind.is_create() || event.kind.is_modify() {
                    let cloned_span = span.clone();
                    let compressed =
                        tokio::task::spawn_blocking(move || -> std::io::Result<u64> {
                            let _handle = cloned_span.enter();
                            let mut i = 0;
                            for path in event.paths {
                                if path.extension().is_some_and(|ext| ext == "gz") {
                                    continue;
                                }
                                info!("{} changed, compressing", path.display());
                                i += compress_epicly(&path)?;
                            }
                            Ok(i)
                        })
                        .await
                        .unwrap()?;

                    if compressed > 0 {
                        let _handle = span.enter();
                        info!(compressed_files=%compressed, "compressed {compressed} files");
                    }
                } else if let EventKind::Remove(remove_event) = event.kind // UNSTABLE
                    && matches!(remove_event, RemoveKind::File)
                {
                    for path in event.paths {
                        if path.extension().is_some_and(|ext| ext == "gz") {
                            continue;
                        }
                        let gz_path = path.clone().append(".gz");
                        if tokio::fs::try_exists(&gz_path).await? {
                            info!(
                                "{} removed, also removing {}",
                                path.display(),
                                gz_path.display()
                            );
                            tokio::fs::remove_file(&gz_path).await?
                        }
                    }
                }
            }
            Err(err) => return Err(err),
        }
    }

    Ok(())
}
