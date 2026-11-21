//! Actions of the forwarding views. The rule editor's are bound under the key context
//! `RuleEditor`, Port forwarding's under `PortAuthority`.

use gpui::actions;

actions!(
    forwarding,
    [
        /// Saves the rule editor's changes.
        SaveRuleEditor,
        /// Closes the rule editor without saving.
        CancelRuleEditor,
        /// Starts the rule under the cursor, or stops it if it's running.
        ToggleForward,
        /// Stops every running forward.
        StopAllForwards,
        /// Opens the editor of the host whose rule is under the cursor, to change its rules.
        EditRuleHost,
    ]
);
