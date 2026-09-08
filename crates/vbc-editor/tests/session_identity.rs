//! The identifiers a session is started under, which the binary will not start without.
//!
//! `--session-id` takes a uuid and nothing else, so an identifier that is nearly one is a session
//! that never starts. The generation these check has no crate behind it and no entropy source of
//! its own -- it is the clock, the process and the hash seeds the standard library already draws
//! from the operating system -- which buys uniqueness and not unpredictability, and uniqueness is
//! the whole of what a name for a local conversation has to be.
//!
//! That the real binary accepts what is generated here is not something a shape check can say.
//! `session_live.rs` starts a session under one and reads the identifier back out of the init
//! frame, which is the only place that is answered.

use anyhow::Result;
use vbc_editor::session::identity::{
    Identity, SessionId, FORK_SESSION_FLAG, RESUME_FLAG, SESSION_ID_FLAG,
};

/// How many identifiers are generated at once when they are checked for repeating themselves.
const GENERATED: usize = 10_000;

#[test]
fn no_identifier_generated_repeats_another() -> Result<()> {
    let generated: Vec<String> = (0..GENERATED)
        .map(|_| SessionId::generated().to_string())
        .collect();

    let mut distinct: Vec<&String> = generated.iter().collect();
    distinct.sort_unstable();
    distinct.dedup();

    assert_eq!(
        generated.len(),
        distinct.len(),
        "two sessions would have been started under one name"
    );

    Ok(())
}

#[test]
fn every_identifier_generated_is_written_the_way_a_uuid_is() -> Result<()> {
    for _ in 0..GENERATED {
        let id = SessionId::generated().to_string();

        assert_eq!(
            36,
            id.len(),
            "`{id}` is not the length a uuid is written at"
        );
        assert_eq!(
            vec![8, 13, 18, 23],
            id.match_indices('-')
                .map(|(at, _)| at)
                .collect::<Vec<usize>>(),
            "`{id}` is not grouped the way a uuid is"
        );
        assert!(
            id.chars()
                .all(|character| '-' == character || character.is_ascii_hexdigit()),
            "`{id}` holds something that is not a hexadecimal digit"
        );
        assert!(
            id.chars().all(|character| !character.is_ascii_uppercase()),
            "`{id}` is not written in lower case"
        );
        assert_eq!(
            Some('4'),
            id.chars().nth(14),
            "`{id}` does not name the version it is"
        );
        assert!(
            matches!(id.chars().nth(19), Some('8' | '9' | 'a' | 'b')),
            "`{id}` does not name the variant it is"
        );
    }

    Ok(())
}

#[test]
fn an_identifier_a_session_is_already_known_by_is_kept_as_it_stands() -> Result<()> {
    let known = "550e8400-e29b-41d4-a716-446655440000";

    assert_eq!(known, SessionId::known(known).as_str());

    Ok(())
}

#[test]
fn each_way_of_joining_a_conversation_is_asked_for_by_its_own_flags() -> Result<()> {
    let id = SessionId::known("550e8400-e29b-41d4-a716-446655440000");

    assert_eq!(
        vec![SESSION_ID_FLAG, id.as_str()],
        Identity::Fresh(id.clone()).arguments()
    );
    assert_eq!(
        vec![RESUME_FLAG, id.as_str()],
        Identity::Resumed(id.clone()).arguments()
    );
    assert_eq!(
        vec![RESUME_FLAG, id.as_str(), FORK_SESSION_FLAG],
        Identity::Forked(id.clone()).arguments()
    );
    assert_eq!(&id, Identity::Forked(id.clone()).session_id());

    Ok(())
}

#[test]
fn a_conversation_asked_for_without_a_name_is_given_one_of_its_own() -> Result<()> {
    let (first, second) = (Identity::default(), Identity::default());

    assert!(matches!(first, Identity::Fresh(_)));
    assert_ne!(first.session_id(), second.session_id());

    Ok(())
}
