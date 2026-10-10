//! Actual MCP writes racing SQLite and the scripted Alpaca engine.
mod common;
mod action_race {
    use super::common;
    use common::{seed, scripted, web::{self as harness, TestClock}};
    use deltabadger::{engine::{model, run, tick, Clock}, web::{App, Config}, venue::{alpaca::{AlpacaVenue, Urls}, http::{HttpRequest, HttpResponse, ScriptedTransport, Transport, TransportError}, VenueFactory}};
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::{cell::{RefCell,Cell}, rc::Rc, sync::{Arc, Mutex, mpsc}, time::Duration};
    use tokio::sync::Notify;
    type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
    const NOW: &str = "2026-09-03T15:00:00Z";
    const ANCHOR: &str = "2026-09-01 10:00:00";
    const LIMIT: Duration = Duration::from_secs(15);


    // rusqlite's safe busy-handler interface takes a function pointer. Serialize only
    // these probes, and keep the callback bounded even if an assertion unwinds.
    static PROBE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    static BUSY: Mutex<Option<(Arc<Notify>, mpsc::Receiver<()>)>> = Mutex::new(None);
    fn busy(attempt: i32) -> bool {
        if attempt != 0 { return false; }
        let Ok(probe) = BUSY.lock() else { return false; };
        let Some((entered, release)) = &*probe else { return false; };
        entered.notify_one();
        release.recv_timeout(LIMIT).is_ok()
    }

    struct Fixture {
        dir: tempfile::TempDir, c: Connection, app: App, sid: String,
        seed: seed::Seeded, id: i64, eth: i64, clock: Arc<TestClock>, wake: Arc<Notify>,
    }
    impl Fixture {
        async fn new() -> Result<Self> {
            let (dir, opened, seed) = common::install_alpaca();
            let c = opened.primary;
            // A real password login supplies the session and rotated CSRF token.
            let hash = deltabadger::crypto::hash_password("Correct-horse-9").map_err(|e| format!("{e:?}"))?;
            c.execute("UPDATE users SET encrypted_password=?1,confirmed_at=created_at,wash_sale_enabled=0", [hash])?;
            let (eth, _) = seed::add_eth_sol(&c, &seed);
            let mut spec = seed::BotSpec::weekly(60.0, ANCHOR).with("interval", json!("day"));
            spec.status = 2;
            spec.transient = json!({"private":{"null":null,"value":"keep"}});
            spec=spec.weights(&[(seed.btc,1.0),(eth,0.0)]);
            let id = seed::insert_bot(&c, &seed, &spec);
            c.execute("UPDATE bots SET label='Race' WHERE id=?1", [id])?;
            c.execute("UPDATE exchange_assets SET updated_at=?1", ["2026-09-03 15:00:00"])?;
            let clock = TestClock::at(NOW);
            let own = Connection::open(dir.path().join("production.sqlite3"))?;
            own.busy_timeout(LIMIT)?;
            own.pragma_update(None, "foreign_keys", true)?;
            let env = |k: &str| match k { "SECRET_KEY_BASE" => Some("engine-test-secret".into()), _ => None };
            let app = App::new(Config::from_env(&env).map_err(|e|format!("{e:?}"))?, &env, own, clock.clone()).map_err(|e|format!("{e:?}"))?;
            // No market data: the figures stay cold, and a subscription asks for no publication (`loading::resubscribed`),
            // so only the actions' own fragments are broadcast.
            let app = app.with_figure_source(deltabadger::web::figure::loading::Source::Disabled).map_err(|e|format!("{e:?}"))?;
            let wake = Arc::new(Notify::new()); app.attach_engine(wake.clone());
            let names=["stop_bot","archive_bot","unarchive_bot","delete_bot","update_bot_settings","start_bot"];
            c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('M4','m4','http://localhost/cb',0,'mcp',?1,?1)",[NOW])?;
            let application=c.last_insert_rowid();
            c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,'m4-race-token','mcp',?3,3600)",(application,seed.user_id,"2026-09-03 15:00:00"))?;
            c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,created_at,updated_at) VALUES (?1,?2,?3,?4,?4)",(seed.user_id,application,json!(names).to_string(),NOW))?;
            let permissions:serde_json::Map<String,Value>=names.iter().map(|n|(n.to_string(),json!(true))).collect();
            c.execute("UPDATE users SET mcp_settings=?1 WHERE id=?2",(json!({"tool_permissions":permissions,"dry_run":false}).to_string(),seed.user_id))?;
            let init=json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"M4","version":"1"}}});
            let request=axum::http::Request::builder().method("POST").uri("/mcp").header("host","localhost:3000").header("authorization","Bearer m4-race-token")
                .header("content-type","application/json").header("accept","application/json, text/event-stream").body(axum::body::Body::from(init.to_string()))?;
            use tower::ServiceExt;
            let response=deltabadger::web::router(app.clone()).oneshot(request).await?;
            let sid=match response.headers().get("mcp-session-id") {
                Some(v)=>v.to_str()?.to_owned(),
                None=>{let status=response.status();let body=axum::body::to_bytes(response.into_body(),100_000).await?;return Err(format!("initialize {status}: {}",String::from_utf8_lossy(&body)).into());}
            };
            c.execute("UPDATE action_mcp_sessions SET initialized=1,status='initialized'",[])?;
            Ok(Self { dir, c, app, sid, seed, id, eth, clock, wake })
        }
        async fn send(&self, method: &str, suffix: &str, fields: &[(&str,&str)]) -> Result<harness::Answer> {
            let name=match (method,suffix) {
                (_,"")=>"update_bot_settings",(_,"/stop")=>"stop_bot",(_,"/delete")=>"delete_bot",
                ("DELETE","/archive")=>"unarchive_bot",(_,"/archive")=>"archive_bot",(_,_)=>"start_bot",
            };
            let mut args=json!({"bot_id":self.id});
            for (key,value) in fields {
                let key=key.strip_prefix("bots_dca_multi_asset[").and_then(|k|k.strip_suffix(']')).unwrap_or(key);
                args[key]=if key=="quote_amount" {json!(value.parse::<f64>()?)} else {json!(value)};
            }
            let payload=json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":args}});
            let request=axum::http::Request::builder().method("POST").uri("/mcp").header("host","localhost:3000").header("authorization","Bearer m4-race-token")
                .header("content-type","application/json").header("accept","application/json, text/event-stream").header("mcp-session-id",&self.sid).body(axum::body::Body::from(payload.to_string()))?;
            use tower::ServiceExt;
            let response=tokio::time::timeout(LIMIT,deltabadger::web::router(self.app.clone()).oneshot(request)).await??;
            let status=response.status().as_u16();
            let body=String::from_utf8(axum::body::to_bytes(response.into_body(),100_000).await?.to_vec())?;
            Ok(harness::Answer{status,body,headers:vec![]})
        }
        async fn ok(&self, method: &str, suffix: &str, fields: &[(&str,&str)]) -> Result<harness::Answer> {
            let answer=self.send(method,suffix,fields).await?;
            let response:Value=serde_json::from_str(&answer.body)?;
            assert_eq!(answer.status,200,"{}",answer.body);
            assert!(response["error"].is_null() && response["result"]["isError"]!=true,"{}",answer.body);
            assert!(!response["result"]["content"][0]["text"].as_str().ok_or("tool text")?.starts_with("Failed"),"{}",answer.body);
            Ok(answer)
        }
        fn one<T: rusqlite::types::FromSql>(&self, sql: &str) -> Result<T> { Ok(self.c.query_row(sql,[self.id],|r|r.get(0))?) }
        fn snapshot(&self) -> Result<Vec<String>> {
            let mut out=vec![];
            for table in ["bots","bot_index_assets","bot_activity_logs","transactions","api_keys","users"] {
                let mut q=self.c.prepare(&format!("SELECT * FROM {table} ORDER BY id"))?;
                let columns=q.column_count(); let mut rows=q.query([])?;
                while let Some(row)=rows.next()? {
                    let values=(0..columns).map(|i|row.get_ref(i).map(|v|format!("{v:?}"))).collect::<std::result::Result<Vec<_>,_>>()?;
                    out.push(format!("{table}:{values:?}"));
                }
            } Ok(out)
        }
        fn evidence(&self,name:&str,answer:&harness::Answer,before:&[String])->Result {
            if let Ok(root)=std::env::var("M4_EVIDENCE") {
                let dir=std::path::Path::new(&root).join("writer-refusals");std::fs::create_dir_all(&dir)?;
                let result:Value=serde_json::from_str(&answer.body)?;
                std::fs::write(dir.join(format!("{name}.json")),serde_json::to_vec_pretty(&json!({"status":answer.status,"body":answer.body,"result":result,"before":before,"after":self.snapshot()?}))?)?;
            } Ok(())
        }
        fn transient(&self) -> Result<Value> { Ok(serde_json::from_str(&self.one::<String>("SELECT transient_data FROM bots WHERE id=?1")?)?) }
        async fn woke(&self, expected: bool) -> Result {
            // Notify stores a permit; a zero-duration poll observes it without a sleep.
            assert_eq!(tokio::time::timeout(Duration::ZERO,self.wake.notified()).await.is_ok(),expected,"post-commit wake"); Ok(())
        }
        async fn contended(&mut self, method: &str, suffix: &str, fields: &[(&str,&str)], edit: impl FnOnce(&Self)->Result, advance: Option<&str>) -> Result<(harness::Answer,Vec<String>)> {
            let _serial=PROBE_LOCK.lock().await;
            self.app.db(|c| { c.busy_handler(Some(busy))?; Ok(()) }).await.map_err(|e|format!("{e:?}"))?;
            let hit=Arc::new(Notify::new()); let (release, receiver)=mpsc::channel();
            *BUSY.lock().map_err(|e|e.to_string())?=Some((hit.clone(),receiver));
            self.c.execute_batch("BEGIN IMMEDIATE")?;
            edit(self)?;
            let before=self.snapshot()?;
            let request=self.send(method,suffix,fields);
            tokio::pin!(request);
            let deadline=tokio::time::Instant::now()+LIMIT;
            tokio::select! {
                result=&mut request => return Err(format!("request finished before SQLite contention: {} {}; writer still holds BEGIN IMMEDIATE",result?.status,"MCP completed").into()),
                _=hit.notified()=>{},
                _=tokio::time::sleep_until(deadline)=>return Err("request did not enter SQLite busy handler; external writer holds BEGIN IMMEDIATE".into()),
            }
            if let Some(now)=advance { self.clock.set(harness::at(now)); }
            self.c.execute_batch("COMMIT")?;
            release.send(())?;
            let answer=tokio::time::timeout_at(deadline,&mut request).await.map_err(|_|"writer committed and released busy handler, request did not finish")??;
            *BUSY.lock().map_err(|e|e.to_string())?=None;
            Ok((answer,before))
        }
    }

    #[tokio::test(flavor="current_thread")]
    async fn settings_after_settings() -> Result {
        let mut f=Fixture::new().await?;
        let (a,_)=f.contended("PATCH","",&[("bots_dca_multi_asset[label]","second tab")],|f|{
            f.c.execute("UPDATE bots SET settings=json_set(settings,'$.quote_amount',20),transient_data=json_set(transient_data,'$.missed_quote_amount','3.75') WHERE id=?1",[f.id])?;Ok(())
        },None).await?;
        assert_eq!(a.status,200,"{}",a.body); f.woke(true).await?;
        assert_eq!(f.one::<f64>("SELECT json_extract(settings,'$.quote_amount') FROM bots WHERE id=?1")?,20.0);
        assert_eq!(f.transient()?["missed_quote_amount"],"3.75");
        assert_eq!(f.one::<String>("SELECT label FROM bots WHERE id=?1")?,"second tab");
        let (a,_)=f.contended("PATCH","",&[("bots_dca_multi_asset[quote_amount]","42")],|_|Ok(()),None).await?;
        assert_eq!(a.status,200); assert_eq!(f.one::<f64>("SELECT json_extract(settings,'$.quote_amount') FROM bots WHERE id=?1")?,42.0); f.woke(true).await
    }
    #[tokio::test(flavor="current_thread")]
    async fn settings_after_engine_json() -> Result {
        let mut f=Fixture::new().await?;
        let (a,_)=f.contended("PATCH","",&[("bots_dca_multi_asset[label]","second tab")],|f| {
            f.c.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.last_action_job_at','2026-09-03T10:00:00Z','$.last_failure_kind','transient','$.mail_private',json('{\"x\":null}'),'$.placement_private',json('[1,\"2\"]')) WHERE id=?1",[f.id])?;Ok(())
        },None).await?;
        assert_eq!(a.status,200,"{}",a.body);
        assert_eq!(f.transient()?,json!({"private":{"null":null,"value":"keep"},"last_action_job_at":"2026-09-03T10:00:00Z","last_failure_kind":"transient","mail_private":{"x":null},"placement_private":[1,"2"],"missed_quote_amount":null,"missed_quote_amount_was_set":null})); f.woke(true).await
    }
    type Submission=(chrono::DateTime<chrono::Utc>,Value);
    #[derive(Clone)]
    struct Script {
        transport: ScriptedTransport, clock: Arc<TestClock>,
        gate: Option<(&'static str,Rc<Notify>,Rc<Notify>)>, gate_held: Rc<Cell<bool>>,
        submissions: Rc<RefCell<Vec<Submission>>>,
    }
    impl Script {
        fn new(clock: Arc<TestClock>) -> Self {
            let transport=scripted::script(json!({"GET /v2/orders/OTX-1":[scripted::ok(json!({"id":"OTX-1","status":"filled","symbol":"BTC/USD","type":"market","side":"buy","notional":"60","qty":null,"filled_qty":"0.0009375","filled_avg_price":"64000","limit_price":null}))]}));
            Self {transport,clock,gate:None,gate_held:Rc::new(Cell::new(false)),submissions:Rc::new(RefCell::new(vec![]))}
        }
        fn venue(&self) -> AlpacaVenue<Self> { AlpacaVenue::new(self.clone(),Urls::for_passphrase(Some("paper"))) }
    }
    impl Transport for Script {
        async fn send(&self, req: &HttpRequest) -> std::result::Result<HttpResponse,TransportError> {
            if req.method=="POST" { self.submissions.borrow_mut().push((self.clock.now(),req.body.clone().unwrap_or(Value::Null))); }
            if let Some((path,entered,release))=&self.gate {
                if req.path==*path && !self.gate_held.replace(true) { entered.notify_one(); tokio::time::timeout(LIMIT,release.notified()).await.map_err(|_|TransportError::Permanent(format!("script held {} awaiting HTTP writer",req.path)))?; }
            }
            self.transport.send(req).await
        }
    }
    impl VenueFactory for Script {
        type V=AlpacaVenue<Self>;
        fn for_bot(&self,_: &str,_: Option<deltabadger::crypto::Credentials>)->Self::V {self.venue()}
    }
    async fn price_race(edit: bool, restart: bool, after_send: bool, stop: &str) -> Result {
        let f=Fixture::new().await?; f.ok("PATCH","/start",&[]).await?; f.woke(true).await?;
        f.clock.set(harness::at("2026-09-03T15:00:00.001Z"));
        let mut script=Script::new(f.clock.clone()); let entered=Rc::new(Notify::new()); let release=Rc::new(Notify::new());
        script.gate=Some((if after_send {"/v2/orders"} else {"/v1beta3/crypto/us/latest/quotes"},entered.clone(),release.clone()));
        let engine_db=Connection::open(f.dir.path().join("production.sqlite3"))?;
        let venue=script.venue(); let mut attempts=tick::Attempts::default();
        let clock=f.clock.clone();
        let future=tick::tick(&engine_db,&venue,f.id,&*clock,&mut attempts); tokio::pin!(future);
        tokio::select! {
            _=entered.notified()=>{},
            result=&mut future=>return Err(format!("engine ended before barrier: {result:?}").into()),
            _=tokio::time::sleep(LIMIT)=>return Err("engine did not reach scripted barrier; HTTP writer not started".into()),
        }
        if !after_send {assert!(f.transient()?.get("rust_placement").is_none());}
        f.ok("PATCH",stop,&[]).await?;
        if edit {
            f.ok("PATCH","",&[("allocations","BTC:0,ETH:100")]).await?;
            if restart { f.ok("PATCH","/start",&[]).await?; f.clock.set(harness::at("2026-09-03T15:00:00.002Z")); }
        }
        release.notify_one();
        let outcome=tokio::time::timeout(LIMIT,&mut future).await?.map_err(|e|format!("{e:?}"))?;
        assert!(f.transient()?.get("rust_placement").is_none(),"{outcome:?}");
        assert_eq!(f.one::<i64>("SELECT status FROM bots WHERE id=?1")?,if restart {1} else if stop=="/archive" {7} else if stop=="/delete" {3} else {2});
        assert_eq!(script.submissions.borrow().len(),usize::from(after_send));
        assert_eq!(f.one::<i64>("SELECT count(*) FROM transactions WHERE bot_id=?1")?,i64::from(after_send));
        if edit {
            let settings:Value=serde_json::from_str(&f.one::<String>("SELECT settings FROM bots WHERE id=?1")?)?;
            assert_eq!(settings["allocations"],json!({f.seed.btc.to_string():0.0,f.eth.to_string():1.0}));
        }
        if after_send {
            deltabadger::engine::polling::sweep(&engine_db,&venue,&model::load_bot(&engine_db,f.id).map_err(|e|format!("{e:?}"))?,f.clock.now()).await.map_err(|e|format!("{e:?}"))?;
            assert_eq!(f.one::<i64>("SELECT external_status FROM transactions WHERE bot_id=?1")?,2);
        }
        let plain=Script::new(f.clock.clone());
        if restart {
            tick::tick(&engine_db,&plain.venue(),f.id,&*f.clock,&mut tick::Attempts::default()).await.map_err(|e|format!("{e:?}"))?;
            assert_eq!(plain.submissions.borrow().len(),1);
            assert_eq!(plain.submissions.borrow().first().ok_or("new composition order")?.1["symbol"],"ETH/USD");
        } else {
            tick::tick(&engine_db,&plain.venue(),f.id,&*f.clock,&mut tick::Attempts::default()).await.map_err(|e|format!("{e:?}"))?;
            assert!(plain.submissions.borrow().is_empty()); assert_eq!(f.one::<i64>("SELECT status FROM bots WHERE id=?1")?,if stop=="/archive" {7} else if stop=="/delete" {3} else {2});
        }
        Ok(())
    }
    #[tokio::test(flavor="current_thread")]
    async fn stop_during_price()->Result {price_race(false,false,false,"/stop").await}
    #[tokio::test(flavor="current_thread")]
    async fn composition_during_price()->Result {price_race(true,false,false,"/stop").await?;price_race(true,true,false,"/stop").await}
    #[tokio::test(flavor="current_thread")]
    async fn stop_during_tick()->Result {price_race(false,false,true,"/stop").await}

    #[tokio::test(flavor="current_thread")]
    async fn archive_during_price()->Result {price_race(false,false,false,"/archive").await}
    #[tokio::test(flavor="current_thread")]
    async fn delete_during_price()->Result {price_race(false,false,false,"/delete").await}

    /// Re-run only the existing recorder's basket fixture and Continue block on a
    /// scratch Rails install. Capture the predicate before Start changes the status,
    /// and retain the actual ActionJob timestamp as well as its decision label.
    async fn rails_continue() -> Result<Value> {
        tokio::task::spawn_blocking(|| -> std::result::Result<Value,String> {
            let dir=common::rails_install();
            let output=dir.path().join("continue.json");
            let code=r###"
require 'json'
require 'active_support/testing/time_helpers'
include ActiveSupport::Testing::TimeHelpers
source = File.read(Rails.root.join('script/rust/record_vectors.rb'))
fixture = source.split("# Alpaca crypto baskets (rust/src/engine/basket.rs)", 2).fetch(1)
fixture = fixture.split("vector_row_id = 0", 2).fetch(0)
eval("# Alpaca crypto baskets (rust/src/engine/basket.rs)" + fixture, TOPLEVEL_BINDING)
record = source.split("continue_row = lambda", 2).fetch(1).split("# The web UI (rust/src/web).", 2).fetch(0)
record = "vectors = {}; continue_row = lambda" + record
record = record.sub('      raise "#{name}: Rails refused the start:', '      within = bot.restarting_within_interval?' + "\n" + '      raise "#{name}: Rails refused the start:')
record = record.sub("'decision' => decision }", "'decision' => decision, 'within' => within, 'job_at' => at&.iso8601(6), 'checkpoint' => bot.next_interval_checkpoint_at.utc.iso8601(6) }")
eval(record + "\nFile.write(ENV.fetch('ACTION_RACE_RECORD'), JSON.generate(vectors.fetch('continue_start')))", TOPLEVEL_BINDING)
"###;
            let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().ok_or("repository root")?;
            let log=std::fs::File::create(dir.path().join("recorder.log")).map_err(|e|e.to_string())?;
            let mut command=std::process::Command::new(root.join("bin/rails"));
            command.current_dir(root).args(["runner",code]).env_remove("DATABASE_URL")
                .env("APP_ROOT_URL","http://localhost:3000").env("RAILS_ENV","test").env("SKIP_TEST_DATABASE","true")
                .env("ACTION_RACE_RECORD",&output).stdout(log.try_clone().map_err(|e|e.to_string())?).stderr(log);
            for (name,file) in [("PRIMARY","production.sqlite3"),("QUEUE","production_queue.sqlite3"),("CACHE","cache.sqlite3"),("CABLE","cable.sqlite3")] {
                command.env(format!("{name}_DATABASE_URL"),format!("sqlite3:{}",dir.path().join(file).display()));
            }
            struct Child(std::process::Child);
            impl Drop for Child { fn drop(&mut self) {let _=self.0.kill();let _=self.0.wait();} }
            let mut child=Child(command.spawn().map_err(|e|e.to_string())?);
            let deadline=std::time::Instant::now()+Duration::from_secs(60);
            loop {
                if let Some(status)=child.0.try_wait().map_err(|e|e.to_string())? {
                    if !status.success() {return Err(std::fs::read_to_string(dir.path().join("recorder.log")).map_err(|e|e.to_string())?);}
                    break;
                }
                if std::time::Instant::now()>=deadline {return Err("Rails Continue recorder exceeded its absolute 60-second deadline".into());}
                std::thread::sleep(Duration::from_millis(10)); // child readiness, never race ordering
            }
            serde_json::from_str(&std::fs::read_to_string(output).map_err(|e|e.to_string())?).map_err(|e|e.to_string())
        }).await?.map_err(Into::into)
    }

    async fn continued(name: &str, waits: bool)->Result {
        let f=Fixture::new().await?;
        f.c.execute("UPDATE bots SET settings=json_set(settings,'$.allocations',json(?1)) WHERE id=?2",(json!({f.seed.btc.to_string():1.0}).to_string(),f.id))?;
        f.c.execute("DELETE FROM bot_index_assets WHERE bot_id=?1",[f.id])?;
        let vectors=rails_continue().await?;
        let row=vectors.as_array().ok_or("continue vectors")?.iter().find(|v|v["name"]==name).ok_or("continue case")?;
        assert_eq!(row["decision"],if waits {"checkpoint"} else {"now"});
        assert_eq!(row["within"],json!(waits));
        assert_eq!(row["job_at"],if waits {json!("2026-09-04T10:00:00.000000Z")} else {Value::Null});
        f.clock.set(harness::at(row["now"].as_str().ok_or("recorded clock")?));
        f.c.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.last_action_job_at',?1) WHERE id=?2",(row["last_action_job_at"].as_str().ok_or("recorded stamp")?,f.id))?;
        for tx in row["rows"].as_array().ok_or("recorded orders")? {seed::insert_row(&f.c,&f.seed,f.id,f.seed.btc,tx);}
        f.ok("PATCH","/start?start_fresh=false",&[]).await?;
        let request=f.transient()?;
        assert_eq!(request["rust_continue_start"],json!({"requested_at":"2026-09-03T15:00:00Z","was_stopped":true}));
        f.woke(true).await?;
        let script=Script::new(f.clock.clone());
        let paths=deltabadger::store::Paths::from_env(&|_|None,f.dir.path());
        let lock=deltabadger::lease::lock(&paths,f.clock.now()).map_err(|e|format!("{e:?}"))?;
        let own=Connection::open(f.dir.path().join("production.sqlite3"))?;
        let mut engine=run::Engine::new(own,script.clone(),seed::cipher(),lock);
        run::step(&mut engine,&*f.clock).await.map_err(|e|format!("{e:?}"))?;
        assert!(f.transient()?.get("rust_continue_start").is_none());
        assert_eq!(f.transient()?["private"],request["private"]);
        if waits {
            assert!(script.submissions.borrow().is_empty());
            f.clock.set(harness::at("2026-09-04T09:59:59.999999Z"));
            run::step(&mut engine,&*f.clock).await.map_err(|e|format!("{e:?}"))?;
            assert!(script.submissions.borrow().is_empty());
            f.clock.set(harness::at("2026-09-04T10:00:00Z"));
            run::step(&mut engine,&*f.clock).await.map_err(|e|format!("{e:?}"))?;
            assert!(script.submissions.borrow().is_empty(), "a checkpoint is due strictly after its instant");
            f.clock.set(harness::at("2026-09-04T10:00:00.000001Z"));
            run::step(&mut engine,&*f.clock).await.map_err(|e|format!("{e:?}"))?;
        }
        assert_eq!(script.submissions.borrow().len(),1,"Rails recorded {name}: {}; real scheduler at {}: transient={}",row["decision"],f.clock.now(),f.transient()?);
        assert_eq!(script.submissions.borrow().first().ok_or("submission")?.0,f.clock.now());
        assert_eq!(f.one::<String>("SELECT started_at FROM bots WHERE id=?1")?,ANCHOR);
        Ok(())
    }
    #[tokio::test(flavor="current_thread")]
    async fn continue_owes_contribution()->Result {continued("owes_its_contribution",false).await}
    #[tokio::test(flavor="current_thread")]
    async fn continue_within_interval()->Result {continued("nothing_owed",true).await}

    /// MCP paper trading on: start_bot is refused before the engine sees it, and nothing is written.
    #[tokio::test(flavor="current_thread")]
    async fn start_refuses_while_mcp_paper_trading_is_on()->Result {
        let f=Fixture::new().await?;
        f.c.execute("UPDATE users SET mcp_settings=json_set(mcp_settings,'$.dry_run',json('true')) WHERE id=?1",[f.seed.user_id])?;
        let before=f.snapshot()?;
        let answer=f.send("PATCH","/start",&[]).await?;
        let response:Value=serde_json::from_str(&answer.body)?;
        assert_eq!(response["result"]["isError"],true,"{}",answer.body);
        let text=response["result"]["content"][0]["text"].as_str().ok_or("paper refusal")?;
        assert!(text.starts_with("[DRY RUN] Paper trading is on"),"{text}");
        assert_eq!(f.snapshot()?,before);
        f.evidence("mcp-paper",&answer,&before)?;
        f.woke(false).await
    }

    /// MCP paper trading off: a live venue key is still refused by the engine, which runs paper accounts only.
    #[tokio::test(flavor="current_thread")]
    async fn start_refuses_live_key()->Result {
        let f=Fixture::new().await?;
        let cipher=seed::cipher();
        f.c.execute("UPDATE api_keys SET passphrase=?1",[cipher.encrypt("live")])?;
        f.c.execute("UPDATE bots SET status=0 WHERE id=?1",[f.id])?;
        let before=f.snapshot()?;
        let answer=f.send("PATCH","/start",&[]).await?;
        let response:Value=serde_json::from_str(&answer.body)?;
        assert_eq!(response["result"]["isError"],true,"{}",answer.body);
        let text=response["result"]["content"][0]["text"].as_str().ok_or("paper refusal")?;
        assert!(text.starts_with("This app can't run that yet:"),"{text}");
        assert!(text.contains("paper"),"{text}");
        assert_eq!(f.snapshot()?,before);
        f.evidence("live-key",&answer,&before)?;
        f.woke(false).await
    }

    #[tokio::test(flavor="current_thread")]
    async fn each_write_rolls_back_on_guard_refusal()->Result {
        for (tool,status) in [("stop_bot",1),("archive_bot",2),("unarchive_bot",7),("delete_bot",2),("update_bot_settings",2),("start_bot",0)] {
            let f=Fixture::new().await?;
            let other=seed::insert_bot(&f.c,&f.seed,&seed::BotSpec::weekly(60.0,ANCHOR).with("price_limited",json!(true)));
            f.c.execute("UPDATE bots SET status=?1 WHERE id=?2",(status,f.id))?;
            let before=f.snapshot()?;
            let (method,suffix)=match tool {"stop_bot"=>("PATCH","/stop"),"archive_bot"=>("POST","/archive"),"unarchive_bot"=>("DELETE","/archive"),"delete_bot"=>("DELETE","/delete"),"update_bot_settings"=>("PATCH",""),_=>("PATCH","/start")};
            let fields=if tool=="update_bot_settings" {vec![("label","Candidate")]} else {vec![]};
            let answer=f.send(method,suffix,&fields).await?;
            let result:Value=serde_json::from_str(&answer.body)?;
            assert_eq!(result["result"]["isError"],true,"{tool}: {}",answer.body);
            let text=result["result"]["content"][0]["text"].as_str().ok_or("guard text")?;
            assert!(text.starts_with("This app can't run that yet:"),"{text}");
            assert!(text.contains(&format!("bot {other}")),"{text}");
            assert_eq!(f.snapshot()?,before,"{tool} rolled back every primary row");
            f.woke(false).await?;
            f.evidence(&format!("guard-{tool}"),&answer,&before)?;
        }
        Ok(())
    }
    #[tokio::test(flavor="current_thread")]
    async fn r1_refusal_branches_report_error_and_rollback()->Result {
        for branch in ["missing_bot","missing_key","pending_key","incorrect_key","fence","validation","unported","conflict","nochange"] {
            let f=Fixture::new().await?;
            let (suffix,expected)=match branch {
                "missing_bot"=>{f.c.execute("UPDATE bots SET status=3 WHERE id=?1",[f.id])?;("/start",false)},
                "missing_key"=>{f.c.execute("DELETE FROM api_keys",[])?;("/start",false)},
                "pending_key"=>{f.c.execute("UPDATE api_keys SET status=0",[])?;("/start",false)},
                "incorrect_key"=>{f.c.execute("UPDATE api_keys SET status=2",[])?;("/start",false)},
                "fence"=>{f.c.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.rust_placement',json('{}')) WHERE id=?1",[f.id])?;("/start",true)},
                "validation"=>{f.c.execute("UPDATE bots SET settings=json_set(settings,'$.rebalance_threshold',0) WHERE id=?1",[f.id])?;("/archive",false)},
                "unported"=>{f.c.execute("UPDATE bots SET transient_data=?2 WHERE id=?1",(f.id," ".repeat(65537)+"{}"))?;("/start",true)},
                "conflict"=>("/stop",false),
                "nochange"=>{f.ok("PATCH","",&[("label","Race")]).await?;f.woke(true).await?;("",false)},
                _=>unreachable!(),
            };
            let before=f.snapshot()?;
            let fields=if branch=="nochange" {vec![("label","Race")]}else{vec![]};
            let answer=f.send("PATCH",suffix,&fields).await?;
            let response:Value=serde_json::from_str(&answer.body)?;
            assert_eq!(response["result"]["isError"],if expected {json!(true)}else{Value::Null},"R1 refusal {branch}: {}",answer.body);
            if matches!(branch,"missing_key"|"pending_key"|"incorrect_key") {
                assert_eq!(response["result"]["content"][0]["text"],"Failed to start bot 'Race': A valid trading API key is required to start this bot.","#507 validation {branch}");
            }
            if branch=="fence" {assert!(response["result"]["content"][0]["text"].as_str().ok_or("fence text")?.contains("an order is still being reconciled"),"R1 fence reason: {}",answer.body);}
            assert_eq!(f.snapshot()?,before,"R1 refusal {branch} committed");
            f.woke(false).await?;
            f.evidence(branch,&answer,&before)?;
        }
        Ok(())
    }
    #[tokio::test(flavor="current_thread")]
    async fn r1_invalid_lifecycle_rows_validate_but_stop()->Result {
        for suffix in ["/stop","/archive","/delete","unarchive"] {
            let f=Fixture::new().await?;
            f.c.execute("UPDATE bots SET status=?2,settings=json_set(settings,'$.rebalance_enabled',json('false'),'$.rebalance_threshold',0) WHERE id=?1",(f.id,if suffix=="unarchive" {7}else{1}))?;
            let before=f.snapshot()?;
            let answer=f.send(if suffix=="unarchive" {"DELETE"}else{"PATCH"},if suffix=="unarchive" {"/archive"}else{suffix},&[]).await?;
            let response:Value=serde_json::from_str(&answer.body)?;
            assert_eq!(response["result"]["isError"],Value::Null,"R1 lifecycle {suffix}: {}",answer.body);
            if suffix=="/stop" {
                assert_eq!(response["result"]["content"][0]["text"],"Bot 'Race' stopped.","R1 lifecycle /stop");
                assert_eq!(f.one::<i64>("SELECT status FROM bots WHERE id=?1")?,2);
                f.woke(true).await?;
                let script=Script::new(f.clock.clone());
                tick::tick(&f.c,&script.venue(),f.id,&*f.clock,&mut tick::Attempts::default()).await.map_err(|e|format!("{e:?}"))?;
                assert!(script.submissions.borrow().is_empty(),"R1 invalid stopped bot placed an order");
            } else {
                assert!(response["result"]["content"][0]["text"].as_str().ok_or("text")?.starts_with("Failed"),"R1 lifecycle {suffix}: {}",answer.body);
                assert_eq!(f.snapshot()?,before,"R1 lifecycle {suffix} committed");f.woke(false).await?;
            }
        } Ok(())
    }

}
