//! Explicit, session-only watching of one selected package. No discovery or scripts.
use super::{
    lifecycle::Reviews,
    manager::{Manager, Status, UiEvent},
    permissions::Source,
    ProtocolError, Result,
};
use notify::Watcher;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio_util::sync::CancellationToken;
struct Selected {
    id: String,
    cancel: CancellationToken,
    _watcher: notify::RecommendedWatcher,
    trigger: tokio::sync::mpsc::Sender<()>,
}
#[derive(Default)]
pub struct Development {
    enabled: AtomicBool,
    selected: Mutex<Option<Selected>>,
}
impl Development {
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
    pub fn package(&self) -> Option<String> {
        self.selected.lock().unwrap().as_ref().map(|s| s.id.clone())
    }
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        if !enabled {
            self.stop(None);
        }
    }
    pub fn stop(&self, id: Option<&str>) {
        let mut selected = self.selected.lock().unwrap();
        if selected
            .as_ref()
            .is_some_and(|s| id.is_none_or(|id| s.id == id))
        {
            if let Some(selected) = selected.take() {
                selected.cancel.cancel();
            }
        }
    }
    pub fn reload(&self) -> Result<()> {
        let selected = self.selected.lock().unwrap();
        let selected = selected
            .as_ref()
            .ok_or_else(|| ProtocolError::invalid("Select a development package first"))?;
        selected
            .trigger
            .try_send(())
            .map_err(|_| ProtocolError::invalid("A development reload is already queued"))
    }
    pub fn watch(
        &self,
        manager: Arc<Manager>,
        reviews: Arc<Reviews>,
        path: PathBuf,
        id: String,
        source: Source,
    ) -> Result<()> {
        if !self.enabled() {
            return Err(ProtocolError::invalid("Developer mode is off"));
        }
        self.stop(None);
        let path = path
            .canonicalize()
            .map_err(|_| ProtocolError::invalid("Development package is unavailable"))?;
        let watched = path.clone();
        let parent = path
            .parent()
            .ok_or_else(|| ProtocolError::invalid("Invalid development package"))?;
        let (trigger, mut changes) = tokio::sync::mpsc::channel(1);
        let notify = trigger.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    if (event.kind.is_modify() || event.kind.is_create())
                        && event.paths.iter().any(|p| p == &watched)
                    {
                        let _ = notify.try_send(());
                    }
                }
            })
            .map_err(|_| ProtocolError::invalid("Cannot watch development package"))?;
        watcher
            .watch(parent, notify::RecursiveMode::NonRecursive)
            .map_err(|_| ProtocolError::invalid("Cannot watch development package"))?;
        let cancel = CancellationToken::new();
        let stopped = cancel.clone();
        let plugin = id.clone();
        tauri::async_runtime::spawn(async move {
            let mut pending = None::<String>;
            loop {
                tokio::select! {biased;_=stopped.cancelled()=>break,event=changes.recv()=>{if event.is_none(){break}
                    // Coalesce filesystem events, including atomic package replacement.
                    tokio::select!{_=stopped.cancelled()=>break,_=tokio::time::sleep(std::time::Duration::from_millis(200))=>{}}
                    while changes.try_recv().is_ok(){}
                    if manager.paused(){continue}
                    let Ok(installation)=manager.installation(&plugin) else{break};
                    if !installation.desired_enabled || !matches!(installation.status,Status::EnabledIdle|Status::EnabledRunning) || installation.source!=source {continue}
                    if let Some(token)=pending.take(){reviews.cancel(&token);}
                    let read_manager=manager.clone(); let read_reviews=reviews.clone(); let read_path=path.clone();
                    let read_plugin=plugin.clone(); let read_source=source.clone(); let read_cancel=stopped.clone();
                    let result=tokio::task::spawn_blocking(move || read_reviews.prepare_reload(&read_manager,&read_path,&read_plugin,read_source,read_cancel)).await
                        .unwrap_or_else(|_|Err(ProtocolError::invalid("Development package validation failed")));
                    match result {
                        Ok(review) if review.digest==installation.digest=>reviews.cancel(&review.token),
                        Ok(review) if review.expands_access=>{pending=Some(review.token.clone());let _=manager.events.send(UiEvent::DevelopmentReview{review});},
                        Ok(review)=>{
                            if stopped.is_cancelled(){reviews.cancel(&review.token);break}
                            if let Err(error)=reviews.accept(&manager,&review.token,true,false).await{let _=manager.events.send(UiEvent::DevelopmentError{message:error.message});}
                        },
                        Err(error)=>{let _=manager.events.send(UiEvent::DevelopmentError{message:error.message});}
                    }
                }}
            }
            if let Some(token) = pending {
                reviews.cancel(&token);
            }
        });
        *self.selected.lock().unwrap() = Some(Selected {
            id,
            cancel,
            _watcher: watcher,
            trigger,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_file_watch_requires_review_for_changed_access_and_stop_revokes_it() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("private"), "unused".into()).unwrap();
        let reviews = Arc::new(Reviews::default());
        let path = root.path().join("selected.cmxaddon");
        std::fs::write(&path, super::super::package::fixture_archive()).unwrap();
        let review = reviews.prepare_local(&manager, &path).unwrap();
        let mut installed = reviews
            .accept(&manager, &review.token, false, false)
            .await
            .unwrap();
        installed.desired_enabled = true;
        installed.status = Status::EnabledIdle;
        manager.save(&installed).unwrap();
        let development = Development::default();
        assert!(development
            .watch(
                manager.clone(),
                reviews.clone(),
                path.clone(),
                installed.manifest.id.clone(),
                installed.source.clone()
            )
            .is_err());
        development.set_enabled(true);
        let mut events = manager.events.subscribe();
        development
            .watch(
                manager.clone(),
                reviews.clone(),
                path.clone(),
                installed.manifest.id.clone(),
                installed.source.clone(),
            )
            .unwrap();
        // The watched package is replaced atomically, as the public CLI packs it.
        let package = super::super::package::Package::read(&path, None).unwrap();
        let mut files = package.files;
        let mut manifest = package.manifest;
        manifest
            .permissions
            .push(codemux_addon_protocol::manifest::Permission::WorkspaceRead);
        files.insert(
            "manifest.json".into(),
            serde_json::to_vec(&manifest).unwrap(),
        );
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (name, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, name, bytes.as_slice())
                .unwrap();
        }
        let bytes = archive.into_inner().unwrap().finish().unwrap();
        let replacement = root.path().join("replacement.cmxaddon");
        std::fs::write(&replacement, bytes).unwrap();
        std::fs::rename(replacement, &path).unwrap();
        let pending = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match events.recv().await.unwrap() {
                    UiEvent::DevelopmentReview { review } => break review,
                    UiEvent::DevelopmentError { message } => panic!("{message}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("native watcher did not observe selected package replacement");
        assert!(pending.expands_access);
        assert!(pending.development);
        assert_eq!(
            manager.installation(&installed.manifest.id).unwrap().digest,
            installed.digest
        );
        development.set_enabled(false);
        assert!(reviews
            .accept(&manager, &pending.token, true, false)
            .await
            .is_err());
        assert!(development.package().is_none());
    }
}
