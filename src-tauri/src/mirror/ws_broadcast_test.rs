use super::*;
use std::cell::Cell;

struct CountedPayload<'a>(&'a Cell<usize>);

impl Serialize for CountedPayload<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.set(self.0.get() + 1);
        serializer.serialize_str("stream chunk")
    }
}

#[test]
fn idle_mirror_skips_serialization_and_connected_clients_receive_the_same_frame() {
    let hub = WsHub::new();
    let calls = Cell::new(0);
    let payload = CountedPayload(&calls);
    hub.broadcast("session://stream", &payload);
    assert_eq!(calls.get(), 0);

    let mut receiver = hub.subscribe();
    hub.broadcast("session://stream", &payload);
    assert_eq!(calls.get(), 1);
    let frame: Value = serde_json::from_str(&receiver.try_recv().unwrap()).unwrap();
    assert_eq!(
        frame,
        json!({
            "type": "event", "name": "session://stream", "payload": "stream chunk"
        })
    );

    drop(receiver);
    hub.broadcast("session://stream", &payload);
    assert_eq!(calls.get(), 1);
}
