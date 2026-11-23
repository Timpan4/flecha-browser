use flecha_permissions::{
    Completion, Decision, Evaluation, Origin, Partition, PermissionError, PermissionKind,
    PermissionManager, PermissionSet, PromptOutcome, RequestId, SiteControl, TabId, Verdict,
};

use PermissionKind::{Camera, ClipboardRead, Geolocation, Microphone, Notifications};

fn origin(url: &str) -> Origin {
    Origin::parse(url).unwrap()
}

fn media() -> PermissionSet {
    SiteControl::Media.kinds()
}

fn prompt(evaluation: Evaluation) -> RequestId {
    match evaluation {
        Evaluation::Prompt(id) => id,
        other => panic!("expected a prompt, got {other:?}"),
    }
}

fn done(request: RequestId, verdict: Verdict) -> Completion {
    Completion { request, verdict }
}

#[test]
fn allow_once_lasts_until_the_tab_leaves_the_page() {
    let mut manager = PermissionManager::new();
    let (tab, other_tab) = (TabId::new("a"), TabId::new("b"));
    let site = origin("https://maps.example");
    let shared = Partition::Shared;

    let id = prompt(manager.evaluate(&shared, &tab, &site, Geolocation));
    let view = manager.pending_prompt(&tab).unwrap();
    assert_eq!((view.request, view.origin), (id, &site));
    assert_eq!(
        manager.resolve_prompt(id, PromptOutcome::AllowOnce),
        Ok(vec![done(id, Verdict::Allow)])
    );
    assert!(!manager.is_dirty());
    assert_eq!(
        manager.site_settings(&shared, &site).decision(Geolocation),
        Decision::Ask
    );
    assert_eq!(
        manager.evaluate(&shared, &tab, &site, Geolocation),
        Evaluation::Resolved(Verdict::Allow)
    );
    // Another tab of the same site still asks.
    prompt(manager.evaluate(&shared, &other_tab, &site, Geolocation));

    assert!(manager.cancel_tab_prompts(&tab).is_empty());
    prompt(manager.evaluate(&shared, &tab, &site, Geolocation));
}

#[test]
fn block_once_blocks_only_the_tab_and_dismiss_asks_again() {
    let mut manager = PermissionManager::new();
    let tab = TabId::new("a");
    let site = origin("https://chat.example");
    let shared = Partition::Shared;

    let id = prompt(manager.evaluate(&shared, &tab, &site, Notifications));
    assert_eq!(
        manager.resolve_prompt(id, PromptOutcome::BlockOnce),
        Ok(vec![done(id, Verdict::Block)])
    );
    assert_eq!(
        manager.evaluate(&shared, &tab, &site, Notifications),
        Evaluation::Resolved(Verdict::Block)
    );
    prompt(manager.evaluate(&shared, &TabId::new("b"), &site, Notifications));

    let id = prompt(manager.evaluate(&shared, &tab, &site, ClipboardRead));
    assert_eq!(
        manager.resolve_prompt(id, PromptOutcome::Dismiss),
        Ok(vec![done(id, Verdict::Block)])
    );
    prompt(manager.evaluate(&shared, &tab, &site, ClipboardRead));
    assert!(!manager.is_dirty());
}

#[test]
fn always_outcomes_are_saved_for_the_origin() {
    let mut manager = PermissionManager::new();
    let site = origin("https://meet.example");
    let shared = Partition::Shared;

    let id = prompt(manager.evaluate(&shared, &TabId::new("a"), &site, Camera));
    manager
        .resolve_prompt(id, PromptOutcome::AllowAlways)
        .unwrap();
    let id = prompt(manager.evaluate(&shared, &TabId::new("a"), &site, Geolocation));
    manager
        .resolve_prompt(id, PromptOutcome::BlockAlways)
        .unwrap();
    assert!(manager.is_dirty());

    let fresh_tab = TabId::new("b");
    assert_eq!(
        manager.evaluate(&shared, &fresh_tab, &site, Camera),
        Evaluation::Resolved(Verdict::Allow)
    );
    assert_eq!(
        manager.evaluate(&shared, &fresh_tab, &site, Geolocation),
        Evaluation::Resolved(Verdict::Block)
    );
    let settings = manager.site_settings(&shared, &site);
    assert_eq!(settings.decision(Camera), Decision::Allow);
    assert_eq!(settings.decision(Geolocation), Decision::Block);
    assert_eq!(settings.control(SiteControl::Media), None);
    assert_eq!(
        settings.control(SiteControl::Notifications),
        Some(Decision::Ask)
    );
}

#[test]
fn compound_media_asks_only_for_the_undecided_kind() {
    let mut manager = PermissionManager::new();
    let tab = TabId::new("a");
    let site = origin("https://meet.example");
    let shared = Partition::Shared;
    manager
        .set_setting(&shared, &site, Camera, Decision::Allow)
        .unwrap();

    let id = prompt(manager.evaluate(&shared, &tab, &site, media()));
    let view = manager.pending_prompt(&tab).unwrap();
    assert_eq!(view.requested, media());
    assert_eq!(view.ask, Microphone.into());
    assert_eq!(
        manager.resolve_prompt(id, PromptOutcome::AllowAlways),
        Ok(vec![done(id, Verdict::Allow)])
    );
    assert_eq!(
        manager
            .site_settings(&shared, &site)
            .control(SiteControl::Media),
        Some(Decision::Allow)
    );
}

#[test]
fn compound_media_is_blocked_when_either_kind_is() {
    let mut manager = PermissionManager::new();
    let tab = TabId::new("a");
    let site = origin("https://meet.example");
    let shared = Partition::Shared;
    manager
        .set_setting(&shared, &site, Microphone, Decision::Block)
        .unwrap();
    assert_eq!(
        manager.evaluate(&shared, &tab, &site, media()),
        Evaluation::Resolved(Verdict::Block)
    );

    manager
        .set_setting(&shared, &site, Microphone, Decision::Ask)
        .unwrap();
    let id = prompt(manager.evaluate(&shared, &tab, &site, media()));
    assert_eq!(manager.pending_prompt(&tab).unwrap().ask, media());
    // A block answer covers both kinds the prompt asked about.
    manager
        .resolve_prompt(id, PromptOutcome::BlockAlways)
        .unwrap();
    let settings = manager.site_settings(&shared, &site);
    assert_eq!(settings.control(SiteControl::Media), Some(Decision::Block));
}

#[test]
fn duplicate_requests_share_one_prompt_and_others_queue() {
    let mut manager = PermissionManager::new();
    let tab = TabId::new("a");
    let site = origin("https://meet.example");
    let shared = Partition::Shared;

    let first = prompt(manager.evaluate(&shared, &tab, &site, media()));
    let duplicate = prompt(manager.evaluate(&shared, &tab, &site, media()));
    let mic_only = prompt(manager.evaluate(&shared, &tab, &site, Microphone));
    let location = prompt(manager.evaluate(&shared, &tab, &site, Geolocation));
    assert_ne!(first, duplicate);
    assert_eq!(manager.pending_prompt(&tab).unwrap().request, first);

    // Answering the media prompt also decides the queued microphone request.
    assert_eq!(
        manager.resolve_prompt(duplicate, PromptOutcome::AllowOnce),
        Ok(vec![
            done(first, Verdict::Allow),
            done(duplicate, Verdict::Allow),
            done(mic_only, Verdict::Allow),
        ])
    );
    assert_eq!(manager.pending_prompt(&tab).unwrap().request, location);
    assert_eq!(
        manager.resolve_prompt(first, PromptOutcome::AllowOnce),
        Err(PermissionError::UnknownRequest)
    );
}

#[test]
fn closing_or_navigating_a_tab_denies_its_pending_prompts() {
    let mut manager = PermissionManager::new();
    let (tab, other_tab) = (TabId::new("a"), TabId::new("b"));
    let site = origin("https://meet.example");
    let shared = Partition::Shared;

    let camera = prompt(manager.evaluate(&shared, &tab, &site, Camera));
    let location = prompt(manager.evaluate(&shared, &tab, &site, Geolocation));
    let elsewhere = prompt(manager.evaluate(&shared, &other_tab, &site, Camera));

    assert_eq!(
        manager.cancel_tab_prompts(&tab),
        vec![done(camera, Verdict::Block), done(location, Verdict::Block)]
    );
    assert!(manager.pending_prompt(&tab).is_none());
    // A late click on the closed tab's prompt changes nothing.
    assert_eq!(
        manager.resolve_prompt(camera, PromptOutcome::AllowAlways),
        Err(PermissionError::UnknownRequest)
    );
    assert_eq!(
        manager.site_settings(&shared, &site).decision(Camera),
        Decision::Ask
    );
    assert!(!manager.is_dirty());
    assert_eq!(
        manager.pending_prompt(&other_tab).unwrap().request,
        elsewhere
    );
}

#[test]
fn withdrawn_requests_leave_the_queue_without_an_answer() {
    let mut manager = PermissionManager::new();
    let tab = TabId::new("a");
    let site = origin("https://meet.example");
    let shared = Partition::Shared;

    let first = prompt(manager.evaluate(&shared, &tab, &site, Camera));
    let second = prompt(manager.evaluate(&shared, &tab, &site, Camera));
    assert!(manager.withdraw(first));
    assert!(!manager.withdraw(first));
    assert_eq!(manager.pending_prompt(&tab).unwrap().request, second);
    assert!(manager.withdraw(second));
    assert!(manager.pending_prompt(&tab).is_none());
}

#[test]
fn site_panel_settings_decide_queued_prompts_and_override_one_time_answers() {
    let mut manager = PermissionManager::new();
    let (tab, other_tab) = (TabId::new("a"), TabId::new("b"));
    let site = origin("https://meet.example");
    let shared = Partition::Shared;

    let id = prompt(manager.evaluate(&shared, &tab, &site, Camera));
    manager
        .resolve_prompt(id, PromptOutcome::AllowOnce)
        .unwrap();
    let queued = prompt(manager.evaluate(&shared, &other_tab, &site, media()));

    assert_eq!(
        manager.set_setting(&shared, &site, SiteControl::Media, Decision::Block),
        Ok(vec![done(queued, Verdict::Block)])
    );
    assert_eq!(
        manager.evaluate(&shared, &tab, &site, Camera),
        Evaluation::Resolved(Verdict::Block)
    );

    // "Ask" forgets the saved decision; the one-time allow is gone too.
    manager
        .set_setting(&shared, &site, SiteControl::Media, Decision::Ask)
        .unwrap();
    assert_eq!(manager.site_settings(&shared, &site), Default::default());
    prompt(manager.evaluate(&shared, &tab, &site, Camera));
}

#[test]
fn clear_origin_forgets_saved_and_one_time_decisions() {
    let mut manager = PermissionManager::new();
    let tab = TabId::new("a");
    let site = origin("https://meet.example");
    let other = origin("https://other.example");
    let shared = Partition::Shared;

    manager
        .set_setting(&shared, &site, Geolocation, Decision::Allow)
        .unwrap();
    manager
        .set_setting(&shared, &other, Geolocation, Decision::Allow)
        .unwrap();
    let id = prompt(manager.evaluate(&shared, &tab, &site, Notifications));
    manager
        .resolve_prompt(id, PromptOutcome::AllowOnce)
        .unwrap();
    manager.mark_saved();

    assert!(manager.clear_origin(&shared, &site).is_empty());
    assert!(manager.is_dirty());
    prompt(manager.evaluate(&shared, &tab, &site, Geolocation));
    prompt(manager.evaluate(&shared, &tab, &site, Notifications));
    assert_eq!(
        manager.evaluate(&shared, &tab, &other, Geolocation),
        Evaluation::Resolved(Verdict::Allow)
    );
}

#[test]
fn spaces_with_separate_cookies_keep_separate_decisions() {
    let mut manager = PermissionManager::new();
    let tab = TabId::new("a");
    let site = origin("https://meet.example");
    let work = Partition::Isolated("work".into());
    let home = Partition::Isolated("home".into());

    manager
        .set_setting(&work, &site, Camera, Decision::Allow)
        .unwrap();
    assert_eq!(
        manager.evaluate(&work, &tab, &site, Camera),
        Evaluation::Resolved(Verdict::Allow)
    );
    prompt(manager.evaluate(&home, &tab, &site, Camera));
    prompt(manager.evaluate(&Partition::Shared, &tab, &site, Camera));
    assert_eq!(
        manager.site_settings(&home, &site).decision(Camera),
        Decision::Ask
    );

    manager.clear_partition(&work);
    prompt(manager.evaluate(&work, &tab, &site, Camera));
}

#[test]
fn one_time_answers_stay_in_their_partition() {
    let mut manager = PermissionManager::new();
    let tab = TabId::new("a");
    let site = origin("https://meet.example");
    let work = Partition::Isolated("work".into());

    let id = prompt(manager.evaluate(&work, &tab, &site, Camera));
    manager
        .resolve_prompt(id, PromptOutcome::AllowOnce)
        .unwrap();
    prompt(manager.evaluate(&Partition::Shared, &tab, &site, Camera));
}

#[test]
fn incognito_decisions_stay_in_memory() {
    let mut manager = PermissionManager::new();
    let tab = TabId::new("a");
    let site = origin("https://meet.example");
    let incognito = Partition::Ephemeral("window-1".into());

    let id = prompt(manager.evaluate(&incognito, &tab, &site, Camera));
    manager
        .resolve_prompt(id, PromptOutcome::AllowAlways)
        .unwrap();
    manager
        .set_setting(&incognito, &site, Geolocation, Decision::Block)
        .unwrap();
    assert_eq!(
        manager.evaluate(&incognito, &TabId::new("b"), &site, Camera),
        Evaluation::Resolved(Verdict::Allow)
    );
    assert!(!manager.is_dirty());
    assert_eq!(manager.snapshot(), Default::default());
    assert!(!manager.snapshot().encode().contains("meet.example"));
    prompt(manager.evaluate(&Partition::Shared, &tab, &site, Camera));

    manager.clear_partition(&incognito);
    prompt(manager.evaluate(&incognito, &tab, &site, Camera));
}

#[test]
fn empty_requests_are_blocked() {
    let mut manager = PermissionManager::new();
    assert_eq!(
        manager.evaluate(
            &Partition::Shared,
            &TabId::new("a"),
            &origin("https://example.com"),
            PermissionSet::EMPTY
        ),
        Evaluation::Resolved(Verdict::Block)
    );
}

#[test]
fn site_panel_names_match_the_chromium_shell() {
    let names: Vec<_> = SiteControl::ALL.iter().map(|c| c.name()).collect();
    assert_eq!(
        names,
        ["geolocation", "media", "notifications", "clipboard-read"]
    );
    assert_eq!(SiteControl::from_name("media"), Some(SiteControl::Media));
    assert_eq!(SiteControl::from_name("camera"), None);
    assert_eq!(
        SiteControl::Media.kinds().iter().collect::<Vec<_>>(),
        [Camera, Microphone]
    );
}

#[test]
fn a_saved_answer_replaces_other_tabs_one_time_answers() {
    let mut manager = PermissionManager::new();
    let (tab, other_tab) = (TabId::new("a"), TabId::new("b"));
    let site = origin("https://meet.example");
    let shared = Partition::Shared;

    let id = prompt(manager.evaluate(&shared, &tab, &site, Camera));
    manager
        .resolve_prompt(id, PromptOutcome::AllowOnce)
        .unwrap();
    let id = prompt(manager.evaluate(&shared, &other_tab, &site, Camera));
    manager
        .resolve_prompt(id, PromptOutcome::BlockAlways)
        .unwrap();

    assert_eq!(
        manager.evaluate(&shared, &tab, &site, Camera),
        Evaluation::Resolved(Verdict::Block)
    );
}

#[test]
fn clearing_a_partition_denies_its_queued_prompts() {
    let mut manager = PermissionManager::new();
    let tab = TabId::new("a");
    let site = origin("https://meet.example");
    let work = Partition::Isolated("work".into());

    let in_work = prompt(manager.evaluate(&work, &tab, &site, Camera));
    let shared = prompt(manager.evaluate(&Partition::Shared, &tab, &site, Camera));
    assert_eq!(
        manager.clear_partition(&work),
        vec![done(in_work, Verdict::Block)]
    );
    assert_eq!(
        manager.resolve_prompt(in_work, PromptOutcome::AllowAlways),
        Err(PermissionError::UnknownRequest)
    );
    assert_eq!(manager.snapshot(), Default::default());
    assert_eq!(manager.pending_prompt(&tab).unwrap().request, shared);
}
