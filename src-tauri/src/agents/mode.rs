//! How much orchestration one turn gets, decided in one place.
//!
//! Three switches overlap (`use_agents`, the per-turn planning switch and the
//! roster's `enabled` flags) plus a fourth one that only matters once a mission
//! exists (`mbus_enabled`). The engine, the planning entry point and the tool
//! table each used to combine them on their own; this module is the single
//! answer, and the truth table below is the contract.

/// What the sub-agent system does for the current turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrchestrationMode {
    /// No sub-agent system: `call_subagent` is neither advertised nor callable.
    Off,
    /// Sub-agents are callable, but nothing is selected or planned up front.
    Manual,
    /// Selection + task plan run before the answer; no mission bus.
    Planned,
    /// Full mission: plan plus a supervising planner on the bus.
    PlannedWithBus,
}

impl OrchestrationMode {
    /// Is `call_subagent` on the tool table?
    pub fn subagents(self) -> bool {
        !matches!(self, OrchestrationMode::Off)
    }

    /// Does the turn run selection + planning before its answer?
    pub fn plans(self) -> bool {
        matches!(
            self,
            OrchestrationMode::Planned | OrchestrationMode::PlannedWithBus
        )
    }

    /// Does a mission created this turn get a message bus and a supervisor?
    pub fn bus(self) -> bool {
        matches!(self, OrchestrationMode::PlannedWithBus)
    }

    /// Stable label for logs.
    pub fn label(self) -> &'static str {
        match self {
            OrchestrationMode::Off => "off",
            OrchestrationMode::Manual => "manual",
            OrchestrationMode::Planned => "planned",
            OrchestrationMode::PlannedWithBus => "planned+bus",
        }
    }
}

/// Resolves the mode. Planning needs agents to exist, so an empty (or fully
/// disabled) roster downgrades to [`OrchestrationMode::Manual`] instead of
/// planning with nobody.
pub fn resolve_orchestration_mode(
    use_agents: bool,
    auto_plan: bool,
    has_enabled_agents: bool,
    mbus_enabled: bool,
) -> OrchestrationMode {
    if !use_agents {
        return OrchestrationMode::Off;
    }
    if !auto_plan || !has_enabled_agents {
        return OrchestrationMode::Manual;
    }
    if mbus_enabled {
        OrchestrationMode::PlannedWithBus
    } else {
        OrchestrationMode::Planned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use OrchestrationMode::*;

    #[test]
    fn off_wins_over_every_other_switch() {
        for auto_plan in [false, true] {
            for has_agents in [false, true] {
                for mbus in [false, true] {
                    assert_eq!(resolve_orchestration_mode(false, auto_plan, has_agents, mbus), Off);
                }
            }
        }
    }

    #[test]
    fn rules_out_planning_that_cannot_run() {
        // No planning switch, nothing to plan.
        assert_eq!(resolve_orchestration_mode(true, false, true, false), Manual);
        assert_eq!(resolve_orchestration_mode(true, false, true, true), Manual);
        // Planning on, but every agent is disabled.
        assert_eq!(resolve_orchestration_mode(true, true, false, true), Manual);
    }

    #[test]
    fn planning_the_whole_matrix() {
        assert_eq!(resolve_orchestration_mode(true, true, true, false), Planned);
        assert_eq!(resolve_orchestration_mode(true, true, true, true), PlannedWithBus);
    }

    #[test]
    fn helpers_agree_with_the_variant() {
        assert!(!Off.subagents() && !Off.plans() && !Off.bus());
        assert!(Manual.subagents() && !Manual.plans() && !Manual.bus());
        assert!(Planned.subagents() && Planned.plans() && !Planned.bus());
        assert!(PlannedWithBus.subagents() && PlannedWithBus.plans() && PlannedWithBus.bus());
    }
}
