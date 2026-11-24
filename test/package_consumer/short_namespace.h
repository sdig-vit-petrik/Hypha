#pragma once

/// Uses only the short namespace `hy`; true when every value reads back.
bool shortNamespaceUsable();

/// Includes hypha with `HYPHA_NO_SHORT_NAMESPACE` beside a global `hy` of its own; true
/// when both read back.
bool ownGlobalHyUsable();
