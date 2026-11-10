//! Everything on screen: gpui windows, views, components, theme and keymap.
//!
//! This is the **view** layer of each feature slice. Views hold state and render; business rules
//! live in `ssherlock-core` and I/O lives in the service and store crates.
#![forbid(unsafe_code)]
