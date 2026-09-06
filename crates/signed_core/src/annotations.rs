use nostr::prelude::*;

/// GitWorkshop and `ngit` cover-note extension, kind 1624.
///
/// A markdown note attached to an issue, patch or PR by its author or a maintainer,
/// not part of the NIP-34 draft, read support for interop.
pub const COVER_NOTE_KIND: Kind = Kind::Custom(1624);

/// Whether a kind-1985 label event is a valid annotation of `root`.
///
/// The event references the root with a lowercase `e` tag,
/// its author must be the root author or a maintainer.
fn label_targets_root(event: &Event, root: &Event, maintainers: &[PublicKey]) -> bool {
    if event.kind != Kind::Label {
        return false;
    }
    if event.pubkey != root.pubkey && !maintainers.contains(&event.pubkey) {
        return false;
    }
    let root_id = root.id.to_hex();
    event
        .tags
        .iter()
        .any(|tag| tag.kind() == "e" && tag.content().is_some_and(|content| content == root_id))
}

/// Whether a kind-1985 label event declares the `#t` namespace,
/// it must also carry at least one `["l", "<value>", "#t"]` label.
fn has_hashtag_labels(event: &Event) -> bool {
    event.tags.iter().any(|tag| tag.as_slice() == ["L", "#t"])
        && event.tags.iter().any(|tag| {
            let slice = tag.as_slice();
            slice.len() >= 3 && slice[0] == "l" && slice[2] == "#t" && !slice[1].is_empty()
        })
}

/// Effective hashtag labels of `root`,
/// the `t` tags on the event itself, self-reported by its author,
/// authorized NIP-32 kind-1985 events in the `#t` namespace add more.
///
/// Labels are additive, so all valid label events contribute,
/// there is no latest-wins semantics.
pub fn labels(root: &Event, label_events: &[Event], maintainers: &[PublicKey]) -> Vec<String> {
    let mut labels: Vec<String> = root
        .tags
        .hashtags()
        .map(|hashtag| hashtag.to_string())
        .collect();

    for event in label_events {
        if !label_targets_root(event, root, maintainers) || !has_hashtag_labels(event) {
            continue;
        }
        for tag in event.tags.iter() {
            let slice = tag.as_slice();
            if slice.len() >= 3 && slice[0] == "l" && slice[2] == "#t" && !slice[1].is_empty() {
                let label = &slice[1];
                if !labels.contains(label) {
                    labels.push(label.clone());
                }
            }
        }
    }

    labels
}

/// Subject or title override of `root` from authorized kind-1985 label events,
/// only label events in the `#subject` namespace count.
///
/// Returns `None` when no valid override exists.
pub fn subject_override(
    root: &Event,
    label_events: &[Event],
    maintainers: &[PublicKey],
) -> Option<String> {
    label_events
        .iter()
        .filter(|event| label_targets_root(event, root, maintainers))
        .filter(|event| {
            event
                .tags
                .iter()
                .any(|tag| tag.as_slice() == ["L", "#subject"])
                && event.tags.iter().any(|tag| {
                    let slice = tag.as_slice();
                    slice.len() >= 3
                        && slice[0] == "l"
                        && slice[2] == "#subject"
                        && !slice[1].is_empty()
                })
        })
        .max_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.to_string().cmp(&b.id.to_string()))
        })
        .and_then(|event| {
            event.tags.iter().find_map(|tag| {
                let slice = tag.as_slice();
                (slice.len() >= 3
                    && slice[0] == "l"
                    && slice[2] == "#subject"
                    && !slice[1].is_empty())
                .then(|| slice[1].clone())
            })
        })
}

/// Effective hashtag labels and subject override of `root` in one pass,
/// mirrors ngit's `get_labels_and_subject`.
pub fn labels_and_subject(
    root: &Event,
    label_events: &[Event],
    maintainers: &[PublicKey],
) -> (Vec<String>, Option<String>) {
    (
        labels(root, label_events, maintainers),
        subject_override(root, label_events, maintainers),
    )
}

/// Effective cover note of `root`.
///
/// Returns `None` when no valid cover note exists.
pub fn cover_note<'a>(
    root: &Event,
    cover_notes: &'a [Event],
    maintainers: &[PublicKey],
) -> Option<&'a Event> {
    let root_id = root.id.to_hex();

    cover_notes
        .iter()
        .filter(|event| {
            event.kind == COVER_NOTE_KIND
                && (event.pubkey == root.pubkey || maintainers.contains(&event.pubkey))
                && event.tags.iter().any(|tag| {
                    tag.kind() == "e" && tag.content().is_some_and(|content| content == root_id)
                })
        })
        .max_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.to_string().cmp(&b.id.to_string()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys_from_hex(hex: &str) -> Keys {
        Keys::new(SecretKey::from_hex(hex).expect("valid secret key"))
    }

    fn signed(author: &Keys, kind: Kind, tags: Vec<Tag>, created_at: u64) -> Event {
        EventBuilder::new(kind, "")
            .tags(tags)
            .custom_created_at(Timestamp::from(created_at))
            .finalize(author)
            .expect("signed event")
    }

    fn root_event() -> Event {
        signed(
            &keys_from_hex("0000000000000000000000000000000000000000000000000000000000000001"),
            Kind::GitIssue,
            vec![Tag::hashtag("bug")],
            100,
        )
    }

    fn e_tag(event: &Event) -> Tag {
        Tag::parse(["e", &event.id.to_hex()]).expect("valid e tag")
    }

    #[test]
    fn labels_take_inline_hashtags_and_external_label_events() {
        let root = root_event();
        let maintainer =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000002");
        let labels_event = signed(
            &maintainer,
            Kind::Label,
            vec![
                e_tag(&root),
                Tag::parse(["L", "#t"]).expect("valid L tag"),
                Tag::parse(["l", "help-wanted", "#t"]).expect("valid l tag"),
            ],
            200,
        );

        let labels = labels(&root, &[labels_event], &[maintainer.public_key()]);

        assert_eq!(labels, vec!["bug", "help-wanted"]);
    }

    #[test]
    fn labels_ignore_unauthorized_and_misnamed_events() {
        let root = root_event();
        let maintainer =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000002");
        let stranger =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000003");

        // A stranger's label event is not authorized.
        let stranger_labels = signed(
            &stranger,
            Kind::Label,
            vec![
                e_tag(&root),
                Tag::parse(["L", "#t"]).expect("valid L tag"),
                Tag::parse(["l", "nope", "#t"]).expect("valid l tag"),
            ],
            200,
        );
        // A valid author referencing a different event.
        let other_labels = signed(
            &maintainer,
            Kind::Label,
            vec![
                Tag::parse([
                    "e",
                    "2222222222222222222222222222222222222222222222222222222222222222",
                ])
                .expect("valid e tag"),
                Tag::parse(["L", "#t"]).expect("valid L tag"),
                Tag::parse(["l", "nope", "#t"]).expect("valid l tag"),
            ],
            200,
        );
        // A valid author without the namespace declaration.
        let missing_namespace = signed(
            &maintainer,
            Kind::Label,
            vec![
                e_tag(&root),
                Tag::parse(["l", "nope", "#t"]).expect("valid l tag"),
            ],
            200,
        );

        assert_eq!(
            labels(
                &root,
                &[stranger_labels, other_labels, missing_namespace],
                &[maintainer.public_key()]
            ),
            vec!["bug"]
        );
    }

    #[test]
    fn subject_override_latest_authorized_event_wins() {
        let root = root_event();
        let maintainer =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000002");
        let older = signed(
            &maintainer,
            Kind::Label,
            vec![
                e_tag(&root),
                Tag::parse(["L", "#subject"]).expect("valid L tag"),
                Tag::parse(["l", "Old title", "#subject"]).expect("valid l tag"),
            ],
            200,
        );
        let newer = signed(
            &maintainer,
            Kind::Label,
            vec![
                e_tag(&root),
                Tag::parse(["L", "#subject"]).expect("valid L tag"),
                Tag::parse(["l", "New title", "#subject"]).expect("valid l tag"),
            ],
            300,
        );

        assert_eq!(
            subject_override(&root, &[newer, older], &[maintainer.public_key()]),
            Some("New title".to_owned())
        );
    }

    #[test]
    fn cover_note_latest_authorized_event_wins() {
        let root = root_event();
        let maintainer =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000002");
        let stranger =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000003");
        let older = signed(&maintainer, COVER_NOTE_KIND, vec![e_tag(&root)], 200);
        let newer = signed(&maintainer, COVER_NOTE_KIND, vec![e_tag(&root)], 300);
        let unauthorized = signed(&stranger, COVER_NOTE_KIND, vec![e_tag(&root)], 400);

        let newer_id = newer.id;
        let events = [older, unauthorized, newer];
        let maintainers = [maintainer.public_key()];
        let note = cover_note(&root, &events, &maintainers);
        assert_eq!(note.map(|event| event.id), Some(newer_id));
    }

    #[test]
    fn cover_note_none_without_valid_events() {
        let root = root_event();

        assert_eq!(cover_note(&root, &[], &[]), None);
    }
}
