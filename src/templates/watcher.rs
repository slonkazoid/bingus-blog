use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use handlebars::{Handlebars, Template};
use notify_debouncer_full::notify::{self, Watcher};
use notify_debouncer_full::{new_debouncer, DebouncedEvent};
use tokio::select;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, trace_span};

use crate::templates::*;

async fn process_event(
    event: DebouncedEvent,
    templates: &mut Vec<(String, Template)>,
) -> Result<(), Box<dyn std::error::Error>> {
    match event.kind {
        notify::EventKind::Create(notify::event::CreateKind::File)
        | notify::EventKind::Modify(_) => {
            for path in &event.paths {
                let span = trace_span!("modify_event", ?path);
                let _handle = span.enter();

                let template_name = match get_template_name(path) {
                    Some(v) => v,
                    None => {
                        trace!("skipping event");
                        continue;
                    }
                };

                trace!("processing recompilation");
                let compiled = compile_path_async_io(path).await?;
                trace!("compiled template {template_name:?}");
                templates.push((template_name.to_owned(), compiled));
            }
        }
        notify::EventKind::Remove(notify::event::RemoveKind::File) => {
            for path in &event.paths {
                let span = trace_span!("remove_event", ?path);
                let _handle = span.enter();

                let (file_name, template_name) = match path
                    .file_name()
                    .and_then(|o| o.to_str())
                    .and_then(|file_name| {
                        get_template_name(Path::new(file_name))
                            .map(|template_name| (file_name, template_name))
                    }) {
                    Some(v) => v,
                    None => {
                        trace!("skipping event");
                        continue;
                    }
                };

                trace!("processing removal");
                let file = TEMPLATES.get_file(file_name);
                if let Some(file) = file {
                    let compiled = compile_included_file(file)?;
                    trace!("compiled template {template_name:?}");
                    templates.push((template_name.to_owned(), compiled));
                }
            }
        }
        _ => {}
    };

    Ok(())
}

pub async fn watch_templates<'a>(
    path: impl AsRef<Path>,
    watcher_token: CancellationToken,
    reg: Arc<RwLock<Handlebars<'a>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let path = path.as_ref();

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    let mut debouncer = new_debouncer(Duration::from_millis(100), None, move |events| {
        tx.blocking_send(events)
            .expect("failed to send message over channel")
    })?;

    debouncer
        .watcher()
        .watch(path, notify::RecursiveMode::NonRecursive)?;

    'event_loop: while let Some(events) = select! {
        _ = watcher_token.cancelled() => {
            debug!("exiting watcher loop");
            break 'event_loop;
        },
        events = rx.recv() => events
    } {
        let events = match events {
            Ok(events) => events,
            Err(err) => {
                error!("error getting events: {err:?}");
                continue;
            }
        };

        let mut templates = Vec::new();

        for event in events {
            trace!("file event: {event:?}");
            if let Err(err) = process_event(event, &mut templates).await {
                error!("error while processing event: {err}");
            }
        }

        let mut reg = reg.write().await;
        for template in templates.into_iter() {
            debug!("registered template {}", template.0);
            reg.register_template(&template.0, template.1);
        }
        drop(reg);
        info!("updated custom templates");
    }

    Ok(())
}
