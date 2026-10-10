//! The order and the production HTTP adapter share one injected clock.
use std::sync::{Arc,Mutex};
use chrono::{DateTime,Utc};
use deltabadger::{engine::{Clock,FixedClock},venue::{Venue,VenueFactory,NewOrder,OrderKind,alpaca::{AlpacaVenue,LiveFactory,Urls},http::{self,ReqwestTransport,HttpRequest,Transport,TransportError}}};
use wiremock::{Mock,MockServer,ResponseTemplate,matchers::{method,path}};

fn order(clock:&dyn Clock)->NewOrder{
    NewOrder{pair:"BTC/USD".into(),kind:OrderKind::Market,volume:"10".into(),quote_volume:true,cl_ord_id:"r8-clock-order".into(),deadline:clock.now()+chrono::Duration::seconds(30),day:false}
}
async fn sends(at:&str){
    let clock=Arc::new(FixedClock(DateTime::parse_from_rfc3339(at).unwrap().with_timezone(&Utc)));
    let server=MockServer::start().await;
    Mock::given(method("POST")).and(path("/v2/orders")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":"r8-accepted"}))).expect(2).mount(&server).await;
    let venue=AlpacaVenue::new(ReqwestTransport::new(http::client(),"key".into(),"secret".into()).with_clock(clock.clone()),Urls{trading:server.uri(),data:server.uri()});
    assert_eq!(venue.add_order(&order(clock.as_ref())).await,Ok("r8-accepted".into()),"R8 local HTTP order sends using the injected clock");
    let factory=LiveFactory::with_paper_boundary(server.uri()).with_clock(clock.clone());
    assert_eq!(factory.for_bot("Exchanges::Alpaca",None).add_order(&order(clock.as_ref())).await,Ok("r8-accepted".into()),"R8 factory carries the same injected clock to HTTP");
    server.verify().await;
}
#[tokio::test(flavor="current_thread")]
async fn r8_fixed_past_clock_order_sends_over_local_http(){sends("2001-01-01T00:00:00Z").await;}
#[tokio::test(flavor="current_thread")]
async fn r8_fixed_future_clock_order_sends_over_local_http(){sends("2201-01-01T00:00:00Z").await;}
struct MovingClock(Mutex<DateTime<Utc>>);
impl Clock for MovingClock{fn now(&self)->DateTime<Utc>{*self.0.lock().unwrap()}}
#[tokio::test(flavor="current_thread")]
async fn r8_expired_injected_bound_refuses_without_sending(){
    let clock=Arc::new(MovingClock(Mutex::new(DateTime::parse_from_rfc3339("2201-01-01T00:00:00Z").unwrap().with_timezone(&Utc))));
    let request_at=clock.now();let server=MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&server).await;
    let transport=ReqwestTransport::new(http::client(),"key".into(),"secret".into()).with_clock(clock.clone());
    let request=HttpRequest{method:"POST",base:server.uri(),path:"/v2/orders".into(),query:vec![],body:None,not_after:Some(request_at+chrono::Duration::seconds(75))};
    *clock.0.lock().unwrap()=request_at+chrono::Duration::seconds(76);
    assert!(matches!(transport.send(&request).await,Err(TransportError::NotSent(_))),"R8 expired bound uses injected time immediately before sending");
    server.verify().await;
}
