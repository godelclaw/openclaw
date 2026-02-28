use crate::policy::GatePolicy;
use crate::types::{Action, AuditEntry, Effect, Stimulus, Verdict};

#[derive(Debug)]
pub struct CoreLoop {
    policy: GatePolicy,
    gas_remaining: u64,
    next_seq: u64,
    audit: Vec<AuditEntry>,
}

impl CoreLoop {
    pub fn new(policy: GatePolicy, gas_budget: u64) -> Self {
        Self {
            policy,
            gas_remaining: gas_budget,
            next_seq: 0,
            audit: Vec::new(),
        }
    }

    pub fn gas_remaining(&self) -> u64 {
        self.gas_remaining
    }

    pub fn audit(&self) -> &[AuditEntry] {
        &self.audit
    }

    pub fn tick<D, E>(&mut self, stimulus: Stimulus, deliberate: D, mut execute: E) -> Vec<Effect>
    where
        D: FnOnce(&Stimulus) -> Vec<Action>,
        E: FnMut(&Action) -> Effect,
    {
        let context = self.policy.context_for_channel(stimulus.channel);
        let schedule_verdict = self.policy.check_schedule(context);
        let mut effects = Vec::new();

        for action in deliberate(&stimulus) {
            if self.gas_remaining == 0 {
                break;
            }

            let gas_before = self.gas_remaining;

            // Chain: schedule -> action-kind -> tool-id -> skill-id -> primitive action checks
            let mut verdict = if !schedule_verdict.is_allowed() {
                schedule_verdict.clone()
            } else {
                let action_kind_verdict = self.policy.check_action_kind(context, action.kind());
                if !action_kind_verdict.is_allowed() {
                    action_kind_verdict
                } else {
                    let tool_identity_verdict =
                        self.policy.check_tool_identity(context, action.tool_name());
                    if !tool_identity_verdict.is_allowed() {
                        tool_identity_verdict
                    } else {
                        let skill_identity_verdict = self
                            .policy
                            .check_skill_identity(context, action.skill_name());
                        if !skill_identity_verdict.is_allowed() {
                            skill_identity_verdict
                        } else {
                            self.policy.check_action(context, &action)
                        }
                    }
                }
            };

            let mut cost = if verdict.is_allowed() {
                Self::action_cost(&action)
            } else {
                1
            };

            if cost > self.gas_remaining {
                verdict = Verdict::deny(format!(
                    "insufficient gas: need {cost}, have {}",
                    self.gas_remaining
                ));
                cost = 1;
            }

            let debit = cost.min(self.gas_remaining);
            self.gas_remaining -= debit;

            let effect = match &verdict {
                Verdict::Allow => execute(action.executable_action()),
                Verdict::Deny { reason } => Effect::Denied {
                    reason: reason.clone(),
                },
            };

            self.audit.push(AuditEntry {
                seq: self.next_seq,
                context,
                stimulus_channel: stimulus.channel,
                stimulus_actor: stimulus.actor.clone(),
                action,
                verdict,
                gas_before,
                gas_after: self.gas_remaining,
            });
            self.next_seq += 1;
            effects.push(effect);
        }

        effects
    }

    fn action_cost(action: &Action) -> u64 {
        match action.executable_action() {
            Action::NoOp { .. } => 0,
            Action::Respond { .. } => 1,
            Action::ReadFile { .. } => 2,
            Action::ListDir { .. } => 2,
            Action::WriteFile { .. } => 3,
            Action::WebFetch { .. } => 4,
            Action::Exec { .. } => 8,
            Action::ToolAction { .. } => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::CoreLoop;
    use crate::config::TimeWindow;
    use crate::policy::{ContextRoots, GatePolicy};
    use crate::types::{Action, Channel, ContextTier, Effect, Stimulus};

    fn make_test_env() -> (PathBuf, ContextRoots) {
        let id = format!(
            "vericore-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos()
        );
        let base = std::env::temp_dir().join(id);
        fs::create_dir_all(base.join("private")).expect("create private");
        fs::create_dir_all(base.join("family")).expect("create family");
        fs::create_dir_all(base.join("repos")).expect("create repos");
        fs::create_dir_all(base.join(".ssh")).expect("create .ssh");
        fs::create_dir_all(base.join(".config/secrets")).expect("create .config/secrets");
        fs::create_dir_all(base.join("photos")).expect("create photos");

        fs::write(base.join("private/diary.txt"), "secret thoughts").expect("seed private file");
        fs::write(base.join("family/plans.txt"), "family dinner friday").expect("seed family file");
        fs::write(base.join("repos/README.md"), "public repo").expect("seed repo file");
        fs::write(base.join(".ssh/id_rsa"), "private key").expect("seed ssh key");
        fs::write(base.join(".config/secrets/token.txt"), "api key").expect("seed token");
        fs::write(base.join("notes.txt"), "public notes").expect("seed notes");
        fs::write(base.join("photos/beach.jpg"), "image data").expect("seed photo");

        let roots =
            ContextRoots::new(&base, base.join("private"), base.join("family")).expect("roots");
        (base, roots)
    }

    fn stim(channel: Channel) -> Stimulus {
        Stimulus {
            channel,
            actor: "test".into(),
            content: "test".into(),
            timestamp: 1,
        }
    }

    fn stub_exec(_: &Action) -> Effect {
        Effect::Executed {
            kind: "stub".into(),
        }
    }

    #[test]
    fn public_can_read_public_file() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::ReadFile {
                    path: base.join("repos/README.md"),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
    }

    #[test]
    fn public_cannot_read_private_file() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::ReadFile {
                    path: base.join("private/diary.txt"),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Denied { .. }));
    }

    #[test]
    fn public_cannot_read_family_file() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::ReadFile {
                    path: base.join("family/plans.txt"),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Denied { .. }));
    }

    #[test]
    fn family_can_read_public_and_family_but_not_private() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramFamily),
            |_| {
                vec![
                    Action::ReadFile {
                        path: base.join("repos/README.md"),
                    },
                    Action::ReadFile {
                        path: base.join("family/plans.txt"),
                    },
                    Action::ReadFile {
                        path: base.join("private/diary.txt"),
                    },
                ]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
        assert!(matches!(effects[1], Effect::Executed { .. }));
        assert!(matches!(effects[2], Effect::Denied { .. }));
    }

    #[test]
    fn private_can_read_everything() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramDm),
            |_| {
                vec![
                    Action::ReadFile {
                        path: base.join("repos/README.md"),
                    },
                    Action::ReadFile {
                        path: base.join("family/plans.txt"),
                    },
                    Action::ReadFile {
                        path: base.join("private/diary.txt"),
                    },
                ]
            },
            stub_exec,
        );
        assert!(effects.iter().all(|e| matches!(e, Effect::Executed { .. })));
    }

    #[test]
    fn dot_paths_are_private_by_default() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![
                    Action::ReadFile {
                        path: base.join(".ssh/id_rsa"),
                    },
                    Action::ReadFile {
                        path: base.join(".config/secrets/token.txt"),
                    },
                ]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Denied { .. }));
        assert!(matches!(effects[1], Effect::Denied { .. }));
    }

    #[test]
    fn private_context_can_read_dot_paths() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramDm),
            |_| {
                vec![Action::ReadFile {
                    path: base.join(".ssh/id_rsa"),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
    }

    #[test]
    fn dot_paths_accessible_when_disabled() {
        let (base, roots) = make_test_env();
        let roots = roots.with_dot_paths_private(false);
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::ReadFile {
                    path: base.join(".ssh/id_rsa"),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
    }

    #[test]
    fn family_prefix_marks_dir_as_family() {
        let (base, roots) = make_test_env();
        let roots = roots
            .with_family_prefixes(vec![base.join("photos")])
            .expect("family prefixes");
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::ReadFile {
                    path: base.join("photos/beach.jpg"),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Denied { .. }));

        let effects = core.tick(
            stim(Channel::TelegramFamily),
            |_| {
                vec![Action::ReadFile {
                    path: base.join("photos/beach.jpg"),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
    }

    #[test]
    fn public_cannot_write_private() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::WriteFile {
                    path: base.join("private/hack.txt"),
                    content: "gotcha".into(),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Denied { .. }));
    }

    #[test]
    fn public_can_write_public() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::WriteFile {
                    path: base.join("notes_new.txt"),
                    content: "hello".into(),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
    }

    #[test]
    fn private_can_write_public_family_and_private() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramDm),
            |_| {
                vec![
                    Action::WriteFile {
                        path: base.join("notes_private_ctx.txt"),
                        content: "public target".into(),
                    },
                    Action::WriteFile {
                        path: base.join("family/family_private_ctx.txt"),
                        content: "family target".into(),
                    },
                    Action::WriteFile {
                        path: base.join("private/private_private_ctx.txt"),
                        content: "private target".into(),
                    },
                ]
            },
            stub_exec,
        );

        assert!(effects.iter().all(|e| matches!(e, Effect::Executed { .. })));
    }

    #[test]
    fn exec_denied_in_public_by_default() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::Exec {
                    command: "ls".into(),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Denied { .. }));
    }

    #[test]
    fn exec_allowed_in_private_when_configured() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]).with_exec_in_private(true);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramDm),
            |_| {
                vec![Action::Exec {
                    command: "ls".into(),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
    }

    #[test]
    fn allowed_host_passes() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec!["openrouter.ai".into()]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::WebFetch {
                    host: "api.openrouter.ai".into(),
                    path: "/v1".into(),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
    }

    #[test]
    fn substring_host_bypass_blocked() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec!["openrouter.ai".into()]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::WebFetch {
                    host: "openrouter.ai.evil.com".into(),
                    path: "/steal".into(),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Denied { .. }));
    }

    #[test]
    fn private_cannot_respond_on_public_channel() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramDm),
            |_| {
                vec![Action::Respond {
                    channel: Channel::TelegramPublic,
                    recipient: "world".into(),
                    content: "secret stuff".into(),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Denied { .. }));
    }

    #[test]
    fn public_can_respond_on_public_channel() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::Respond {
                    channel: Channel::TelegramPublic,
                    recipient: "visitor".into(),
                    content: "hello!".into(),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
    }

    #[test]
    fn path_outside_home_denied() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramDm),
            |_| {
                vec![Action::ReadFile {
                    path: PathBuf::from("/etc/passwd"),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Denied { .. }));
    }

    #[test]
    fn gas_decreases_monotonically() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 10);

        core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![
                    Action::ReadFile {
                        path: base.join("repos/README.md"),
                    },
                    Action::ReadFile {
                        path: base.join("notes.txt"),
                    },
                ]
            },
            stub_exec,
        );

        assert!(core.gas_remaining() < 10);
        for entry in core.audit() {
            assert!(entry.gas_after <= entry.gas_before);
        }
    }

    #[test]
    fn gas_exhaustion_stops_processing() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 3);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![
                    Action::ReadFile {
                        path: base.join("repos/README.md"),
                    },
                    Action::ReadFile {
                        path: base.join("notes.txt"),
                    },
                ]
            },
            stub_exec,
        );

        assert_eq!(effects.len(), 2);
        assert!(matches!(effects[0], Effect::Executed { .. }));
        assert!(matches!(effects[1], Effect::Denied { .. }));
    }

    #[test]
    fn every_action_is_audited() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec!["openrouter.ai".into()]);
        let mut core = CoreLoop::new(policy, 100);

        core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![
                    Action::ReadFile {
                        path: base.join("repos/README.md"),
                    },
                    Action::ReadFile {
                        path: base.join("private/diary.txt"),
                    },
                    Action::WebFetch {
                        host: "evil.com".into(),
                        path: "/".into(),
                    },
                ]
            },
            stub_exec,
        );

        assert_eq!(core.audit().len(), 3);
        assert!(core.audit()[0].verdict.is_allowed());
        assert!(!core.audit()[1].verdict.is_allowed());
        assert!(!core.audit()[2].verdict.is_allowed());
    }

    #[test]
    fn action_kind_gating_off_allows_normal_path() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::ReadFile {
                    path: base.join("repos/README.md"),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
    }

    #[test]
    fn action_kind_gating_denies_unlisted_kind() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]).with_tool_gating(
            true,
            vec!["read_file".into()],
            vec!["*".into()],
            vec!["*".into()],
        );
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::Exec {
                    command: "ls".into(),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Denied { .. }));
    }

    #[test]
    fn action_kind_gating_allows_listed_kind() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]).with_tool_gating(
            true,
            vec!["read_file".into()],
            vec!["*".into()],
            vec!["*".into()],
        );
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::ReadFile {
                    path: base.join("repos/README.md"),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
    }

    #[test]
    fn tool_identity_gating_denies_unlisted_tool() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]).with_tool_identity_gating(
            true,
            vec!["web_search".into()],
            vec!["*".into()],
            vec!["*".into()],
        );
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::ToolAction {
                    tool_name: "read_file".into(),
                    skill_name: None,
                    action: Box::new(Action::ReadFile {
                        path: base.join("repos/README.md"),
                    }),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Denied { .. }));
    }

    #[test]
    fn tool_identity_gating_allows_listed_tool() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]).with_tool_identity_gating(
            true,
            vec!["read_file".into()],
            vec!["*".into()],
            vec!["*".into()],
        );
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::ToolAction {
                    tool_name: "read_file".into(),
                    skill_name: None,
                    action: Box::new(Action::ReadFile {
                        path: base.join("repos/README.md"),
                    }),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
    }

    #[test]
    fn skill_identity_gating_denies_missing_skill() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]).with_skill_identity_gating(
            true,
            vec!["safe_skill".into()],
            vec!["*".into()],
            vec!["*".into()],
        );
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::ToolAction {
                    tool_name: "read_file".into(),
                    skill_name: None,
                    action: Box::new(Action::ReadFile {
                        path: base.join("repos/README.md"),
                    }),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Denied { .. }));
    }

    #[test]
    fn skill_identity_gating_allows_listed_skill() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]).with_skill_identity_gating(
            true,
            vec!["safe_skill".into()],
            vec!["*".into()],
            vec!["*".into()],
        );
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::ToolAction {
                    tool_name: "read_file".into(),
                    skill_name: Some("safe_skill".into()),
                    action: Box::new(Action::ReadFile {
                        path: base.join("repos/README.md"),
                    }),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
    }

    #[test]
    fn no_schedule_always_allowed() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        assert!(
            policy
                .check_schedule_at(ContextTier::Public, 0)
                .is_allowed()
        );
        assert!(
            policy
                .check_schedule_at(ContextTier::Public, 86399)
                .is_allowed()
        );
    }

    #[test]
    fn schedule_allows_within_window() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]).with_schedule(
            0,
            Some(TimeWindow {
                allowed_start_hour: 8,
                allowed_end_hour: 22,
            }),
            None,
            None,
        );
        assert!(
            policy
                .check_schedule_at(ContextTier::Public, 43200)
                .is_allowed()
        );
    }

    #[test]
    fn schedule_denies_outside_window() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]).with_schedule(
            0,
            Some(TimeWindow {
                allowed_start_hour: 8,
                allowed_end_hour: 22,
            }),
            None,
            None,
        );
        assert!(
            !policy
                .check_schedule_at(ContextTier::Public, 10800)
                .is_allowed()
        );
    }

    #[test]
    fn schedule_respects_utc_offset() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]).with_schedule(
            1,
            Some(TimeWindow {
                allowed_start_hour: 8,
                allowed_end_hour: 22,
            }),
            None,
            None,
        );
        assert!(
            policy
                .check_schedule_at(ContextTier::Public, 27000)
                .is_allowed()
        );
        assert!(
            !policy
                .check_schedule_at(ContextTier::Public, 23400)
                .is_allowed()
        );
    }

    #[test]
    fn schedule_midnight_wrap() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]).with_schedule(
            0,
            Some(TimeWindow {
                allowed_start_hour: 22,
                allowed_end_hour: 6,
            }),
            None,
            None,
        );
        assert!(
            policy
                .check_schedule_at(ContextTier::Public, 82800)
                .is_allowed()
        );
        assert!(
            policy
                .check_schedule_at(ContextTier::Public, 10800)
                .is_allowed()
        );
        assert!(
            !policy
                .check_schedule_at(ContextTier::Public, 43200)
                .is_allowed()
        );
    }

    #[test]
    fn schedule_only_affects_configured_tier() {
        let (_base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]).with_schedule(
            0,
            Some(TimeWindow {
                allowed_start_hour: 8,
                allowed_end_hour: 22,
            }),
            None,
            None,
        );
        assert!(
            !policy
                .check_schedule_at(ContextTier::Public, 10800)
                .is_allowed()
        );
        assert!(
            policy
                .check_schedule_at(ContextTier::Private, 10800)
                .is_allowed()
        );
    }
    #[test]
    fn list_dir_allowed_on_public_directory() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::ListDir {
                    path: base.join("repos"),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Executed { .. }));
    }

    #[test]
    fn list_dir_denied_on_private_directory_from_public() {
        let (base, roots) = make_test_env();
        let policy = GatePolicy::new(roots, vec![]);
        let mut core = CoreLoop::new(policy, 100);

        let effects = core.tick(
            stim(Channel::TelegramPublic),
            |_| {
                vec![Action::ListDir {
                    path: base.join("private"),
                }]
            },
            stub_exec,
        );
        assert!(matches!(effects[0], Effect::Denied { .. }));
    }
}
