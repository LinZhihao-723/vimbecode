//! What the client refuses to say, and what it makes of what is said back.
//!
//! The protocol has no error channel in the direction we write. A frame missing `"type":"user"` is
//! dropped by the session with no error frame, no acknowledgement and no exit code, so the failure
//! these tests are about is one that cannot be observed from the outside at all: the only evidence
//! of it is a reply that never comes, which is indistinguishable from a turn still being thought
//! about. That is why the check is on this side and why it is asserted here -- an encoder that
//! stopped refusing an untyped frame would not fail anywhere else, ever.
//!
//! The other direction has the same shape from the other end. A line the session wrote that will
//! not decode is a line we must report rather than skip, because a client that quietly dropped
//! what it could not read would show a turn with a hole in it and call it a turn.

use anyhow::Result;
use serde_json::{json, Value};
use vbc_editor::session::error::Error;
use vbc_editor::session::event::{Event, Kind};
use vbc_editor::session::frame::{Frame, TYPE_FIELD, USER_TYPE};

#[test]
fn a_frame_that_names_no_type_is_refused() -> Result<()> {
    let untyped = json!({"message": {"role": "user", "content": "hello"}});

    let Err(Error::Untyped { frame }) = Frame::checked(untyped.clone()) else {
        panic!("a frame naming no type was accepted, and the session drops those in silence");
    };
    assert_eq!(untyped.to_string(), frame);

    Ok(())
}

#[test]
fn a_frame_whose_type_is_not_a_name_is_refused() -> Result<()> {
    let refused = [
        json!({TYPE_FIELD: "", "message": "empty"}),
        json!({TYPE_FIELD: 7, "message": "a number"}),
        json!({TYPE_FIELD: null, "message": "nothing"}),
        json!([{TYPE_FIELD: USER_TYPE}]),
        json!(USER_TYPE),
    ];

    for value in refused {
        assert!(
            matches!(Frame::checked(value.clone()), Err(Error::Untyped { .. })),
            "`{value}` was accepted as a frame, and the session drops it without reporting it"
        );
    }

    Ok(())
}

#[test]
fn the_refusal_says_the_session_would_not_have_reported_it() -> Result<()> {
    let Err(error) = Frame::checked(json!({"message": "hello"})) else {
        panic!("a frame naming no type was accepted");
    };

    let said = error.to_string();
    assert!(
        said.contains("drop") && said.contains("without reporting"),
        "the refusal does not say why it is raised here rather than by the session: {said}"
    );

    Ok(())
}

#[test]
fn a_message_is_framed_as_a_turn_taken_by_the_user() -> Result<()> {
    let frame = Frame::user("what is my favourite colour?");

    assert_eq!(
        &json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": "what is my favourite colour?"}],
            },
        }),
        frame.value()
    );

    Ok(())
}

#[test]
fn a_frame_is_one_line_however_many_the_message_holds() -> Result<()> {
    let frame = Frame::user("first\nsecond\r\nthird\n");

    let line = frame.line();
    assert_eq!(1, line.matches('\n').count());
    assert!(line.ends_with('\n'));

    let read_back: Value = serde_json::from_str(line.trim_end_matches('\n'))?;
    assert_eq!(
        Some(&json!("first\nsecond\r\nthird\n")),
        read_back.pointer("/message/content/0/text"),
        "the newlines the message holds were framed rather than escaped, so the session reads \
         four frames where one was sent"
    );

    Ok(())
}

#[test]
fn a_line_the_session_wrote_that_is_not_a_frame_is_reported() -> Result<()> {
    let refused = ["not json at all", "[1, 2, 3]", "{\"subtype\": \"init\"}"];

    for line in refused {
        assert!(
            matches!(Event::decoded(line), Err(Error::Undecodable { .. })),
            "`{line}` decoded as a frame"
        );
    }

    Ok(())
}

#[test]
fn a_frame_this_milestone_does_not_read_keeps_what_it_arrived_with() -> Result<()> {
    let event = Event::decoded(r#"{"type":"stream_event","session_id":"s","delta":{"text":"h"}}"#)?;

    assert_eq!(&Kind::Other("stream_event".to_owned()), event.kind());
    assert_eq!(Some("s"), event.session_id());
    assert_eq!(
        Some(&json!("h")),
        event.raw().pointer("/delta/text"),
        "an event dropped what it did not read, so a later milestone cannot read it either"
    );

    Ok(())
}
