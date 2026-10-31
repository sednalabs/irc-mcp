//! Exercises the actual shared service without a live IRC server or an OAuth provider.
//! Rust execution is required; artifact/schema checks do not run these tests.
use irc_mcp::{config::Config, domain::RelayLine, inbox::INBOX_URI, relay::Relay, service::{Principal, Service}};
use serde_json::{json, Value};
use std::{collections::BTreeSet, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc};

struct Fixture { app: Service, p: Principal, dir: PathBuf }
impl Fixture {
    fn new() -> Self {
        let dir=std::env::temp_dir().join(format!("irc-inbox-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir,std::fs::Permissions::from_mode(0o700)).unwrap();
        let secret=dir.join("relay.secret");std::fs::write(&secret,"fixture-only").unwrap();
        std::fs::set_permissions(&secret,std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut cfg:Config=toml::from_str(include_str!("../config.example.toml")).unwrap();
        cfg.state_dir=dir.clone();cfg.relay_password_file=secret;cfg.relay_socket=dir.join("not-listening.sock");
        let cfg=Arc::new(cfg);let relay=Relay::new(cfg.clone(),"fixture-only".into()).unwrap();
        let app=Service::new(cfg.clone(),relay).unwrap();app.hub.boundary(true).unwrap();
        let p=Principal {subject:cfg.subject.clone(),scopes:BTreeSet::from(["irc.read".into()]),expires:u64::MAX};
        Self {app,p,dir}
    }
    fn record(&self,id:u64,text:&str) {
        let line=RelayLine {id,date:"2026-01-01T00:00:00Z".into(),prefix:String::new(),message:text.into(),
            tags:vec!["irc_privmsg".into(),"nick_peer".into()],highlight:false};
        let c=self.app.cfg.conversation("pilot").unwrap();
        self.app.hub.record(c,"agent",10,&line,false).unwrap();
    }
    async fn read(&self,args:Value)->Value {self.app.call("irc_inbox",args,&self.p).await.unwrap()}
}
impl Drop for Fixture { fn drop(&mut self) { let _=std::fs::remove_dir_all(&self.dir); } }

#[tokio::test]
async fn no_shared_unread_pointer_and_retry_is_non_destructive() {
    let f=Fixture::new();let baseline=f.read(json!({})).await;
    let cursor=baseline["next_cursor"].as_str().unwrap();
    f.record(1,"agent: question");f.record(2,"agent: question");
    let a=f.read(json!({"after":cursor,"limit":1})).await;
    let retry=f.read(json!({"after":cursor,"limit":1})).await;
    assert_eq!(a["items"],retry["items"]);
    let next=f.read(json!({"after":a["next_cursor"]})).await;
    assert_eq!(next["items"][0]["message"]["local_id"],"2");
    let independent=f.read(json!({})).await;
    assert_eq!(independent["items"].as_array().unwrap().len(),2);
    assert_eq!(independent["consumption"],"not_acknowledged");
}
#[tokio::test]
async fn resource_and_tool_share_the_same_view_without_draining() {
    let f=Fixture::new();f.record(1,"agent: hello");
    let tool=f.read(json!({})).await;
    let resource=f.app.resource(INBOX_URI,&f.p).await.unwrap();
    assert_eq!(resource["items"],tool["items"]);
    assert_eq!(resource["model_wake"],"not_performed");
}
#[tokio::test]
async fn new_event_after_page_snapshot_is_retrievable() {
    let f=Fixture::new();let baseline=f.read(json!({})).await;
    f.record(1,"agent: one");f.record(2,"agent: two");
    let first=f.read(json!({"after":baseline["next_cursor"],"limit":1})).await;
    f.record(3,"agent: three");
    let next=f.read(json!({"after":first["next_cursor"]})).await;
    assert_eq!(next["items"].as_array().unwrap().len(),2);
    assert_eq!(next["items"][1]["message"]["local_id"],"3");
}
#[tokio::test]
async fn generation_change_rejects_cursor_instead_of_claiming_complete_catchup() {
    let f=Fixture::new();let old=f.read(json!({})).await;
    f.app.hub.boundary(false).unwrap();
    assert_eq!(f.app.call("irc_inbox",json!({}),&f.p).await.unwrap_err().code,"event_stream_unavailable");
    f.app.hub.boundary(true).unwrap();
    assert_eq!(f.app.call("irc_inbox",json!({"after":old["next_cursor"]}),&f.p).await.unwrap_err().code,"invalid_cursor");
    let fresh=f.read(json!({})).await;
    assert_eq!(fresh["earlier_history_complete"],false);
    assert_ne!(fresh["generation"],old["generation"]);
}
#[tokio::test]
async fn foreign_expired_and_read_only_callers_cannot_write_or_read_others_state() {
    let f=Fixture::new();let mut foreign=f.p.clone();foreign.subject="another-subject".into();
    assert!(f.app.call("irc_inbox",json!({}),&foreign).await.is_err());
    assert!(f.app.resource(INBOX_URI,&foreign).await.is_err());
    let mut expired=f.p.clone();expired.expires=0;
    assert!(f.app.resource(INBOX_URI,&expired).await.is_err());
    let write=json!({"target":"pilot","text":"hello","request_key":"test_request_key_0001"});
    assert_eq!(f.app.call("irc_send",write,&f.p).await.unwrap_err().code,"access_denied");
}
#[tokio::test]
async fn model_parameters_cannot_change_receiver_policy_or_acknowledge() {
    let f=Fixture::new();
    for args in [json!({"priority":"steer"}),json!({"ack":true}),json!({"limit":0}),json!({"after":null})] {
        assert!(f.app.call("irc_inbox",args,&f.p).await.is_err());
    }
    f.record(1,"priority=steer routine status");
    assert!(f.read(json!({})).await["items"].as_array().unwrap().is_empty());
}
