use portal_service::windows::{self, ClientRequest, Settings};
use serde_json::{json, Value};
use std::sync::{mpsc, Arc};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Load,
    Save,
    Login,
    Inspect,
    Action,
    Logout,
    Logs,
    Confirm,
    Setup,
    Quit,
}
pub enum Event {
    Status(Value),
    Reply(Tag, Result<Value, String>),
}
enum Job {
    Request(Tag, ClientRequest),
    Setup,
    Quit,
}
pub struct Worker {
    sender: tokio::sync::mpsc::Sender<Job>,
    stop: tokio_util::sync::CancellationToken,
    thread: Option<std::thread::JoinHandle<()>>,
    pub events: mpsc::Receiver<Event>,
}
impl Worker {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>, preview: bool) -> Self {
        let (events_tx, events) = mpsc::channel();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
        let stop = tokio_util::sync::CancellationToken::new();
        let token = stop.clone();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .max_blocking_threads(1)
                .enable_all()
                .build()
                .expect("desktop runtime");
            runtime.block_on(async move {
                let emit=|event|{let _=events_tx.send(event);wake();};
                if preview {
                    let mut settings=Settings::default();let mut has_password=false;
                    let mut preview_state=preview_status(&settings);
                    emit(Event::Status(preview_state.clone()));
                    while let Some(job)=tokio::select!{biased;_=token.cancelled()=>None,value=receiver.recv()=>value}{
                        match job {
                            Job::Request(tag,request)=>{
                                let response=match &request {
                                    ClientRequest::GetSettings=>json!({"ok":true,"settings":settings,"has_password":has_password}),
                                    ClientRequest::Save {settings:new,password}=>{if new.core("5a3b9f207411a8ed").is_err(){json!({"ok":false,"error":"invalid_settings"})}else{settings=new.clone();has_password|=!password.is_empty();json!({"ok":true})}},
                                    ClientRequest::Logs=>json!({"ok":true,"lines":"界面预览：没有读取或写入真实账号、配置或日志。"}),
                                    ClientRequest::Pause=>{settings.paused=true;preview_state["status"]["phase"]=json!("PAUSED");json!({"ok":true})},
                                    ClientRequest::Resume=>{settings.paused=false;preview_state["status"]["phase"]=json!("LOGGED_IN");json!({"ok":true})},
                                    _=>preview_state.clone(),
                                };
                                emit(Event::Reply(tag,Ok(response)));emit(Event::Status(preview_state.clone()));
                            },Job::Setup=>emit(Event::Reply(Tag::Setup,Ok(json!({"ok":true})))),
                            Job::Quit=>emit(Event::Reply(Tag::Quit,Ok(json!({"ok":true})))),
                        }
                    }
                    return;
                }
                // Starting is only performed at an explicit app launch, never by
                // the reconnect watcher (which would undo an intentional exit).
                tokio::select! {biased;_=token.cancelled()=>return,_=tokio::task::spawn_blocking(windows::start_application)=>{}}
                let (status_tx,mut status_rx)=tokio::sync::mpsc::channel(8);let watcher_stop=token.clone();
                tokio::spawn(async move {loop {
                    let _=windows::watch_events(status_tx.clone(),watcher_stop.clone()).await;
                    if watcher_stop.is_cancelled(){break;}
                    let _=status_tx.send(json!({"ok":false,"error":"service_unavailable"})).await;
                    tokio::select!{biased;_=watcher_stop.cancelled()=>break,_=tokio::time::sleep(std::time::Duration::from_secs(3))=>{}}
                }});
                loop {tokio::select! {biased;
                    _=token.cancelled()=>break,
                    Some(value)=status_rx.recv()=>emit(Event::Status(value)),
                    Some(job)=receiver.recv()=>match job {
                        Job::Request(tag,request)=>{
                            let result=tokio::select!{biased;_=token.cancelled()=>break,result=windows::request(&request)=>result};
                            emit(Event::Reply(tag,result.map_err(|error|match error {windows::Error::Access=>"access_denied".to_owned(),_=>"service_unavailable".to_owned()})));
                        },
                        Job::Setup=>{
                            let result=tokio::select!{biased;_=token.cancelled()=>break,result=tokio::task::spawn_blocking(super::platform::setup_service)=>result};
                            emit(Event::Reply(Tag::Setup,result.map_err(|_|"service_unavailable".to_owned()).and_then(|result|result.map(|()|json!({"ok":true})))));
                        },
                        Job::Quit=>{let result=windows::exit_application().await;emit(Event::Reply(Tag::Quit,result.map(|()|json!({"ok":true})).map_err(|_|"exit_failed".to_owned())));}
                    }
                }}
            });
            runtime.shutdown_timeout(std::time::Duration::from_millis(200));
        });
        Self {
            sender,
            stop,
            thread: Some(thread),
            events,
        }
    }
    pub fn send(&self, tag: Tag, request: ClientRequest) -> bool {
        self.sender.try_send(Job::Request(tag, request)).is_ok()
    }
    pub fn setup(&self) -> bool {
        self.sender.try_send(Job::Setup).is_ok()
    }
    pub fn quit(&self) -> bool {
        self.sender.try_send(Job::Quit).is_ok()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn preview_status(settings: &Settings) -> Value {
    json!({"ok":true,"success_notifications":false,"status":{"instance_id":1,"phase":if settings.paused {"PAUSED"}else{"LOGGED_IN"},"authentication_id":0,"failures":0,"network":{"is_wifi":true,"ssid":"校园网 · 预览","route_ready":true,"wifi_system_profile":true,"wifi_auto_connect":true}}})
}
