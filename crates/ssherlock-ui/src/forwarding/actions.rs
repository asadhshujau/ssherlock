//! Actions of the forwarding views. The rule editor's are bound under the key context
//! `RuleEditor`.

use gpui::actions;

actions!(
    forwarding,
    [
        /// Saves the rule editor's changes.
        SaveRuleEditor,
        /// Closes the rule editor without saving.
        CancelRuleEditor,
    ]
);
