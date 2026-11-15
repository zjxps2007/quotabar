# AccessKit consumer library

This library is used by code that consumes AccessKit accessibility trees, such as platform adapters. It does not need to be used directly by applications integrating AccessKit.

QuotaBar vendors version 0.31.0 at upstream commit `5801c4a49ff9b2c9ec3f77b3c31272d0da4c5cd3`. Its change in `src/node.rs` allows a scrollbar to expose a writable range to Windows UI Automation.
