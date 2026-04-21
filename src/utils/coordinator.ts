/**
 * Helpers for the top-level Launch / Claim coordinator dialogs.
 *
 * Kept out of `SessionLauncher.tsx` so the argument-cleaning rules are unit
 * testable without mounting React. The Tauri commands treat `null` as "use
 * the CLI default" and empty strings as explicit overrides — form inputs
 * should not accidentally send an empty override when the user left a field
 * blank, so every value runs through `cleanFormArg` first.
 */

export function cleanFormArg(value: string): string | null {
  const trimmed = value.trim();
  return trimmed.length === 0 ? null : trimmed;
}

export interface LaunchFormValues {
  name: string;
  briefing: string;
  sharedDir: string;
  model: string;
}

export interface LaunchInvokeArgs {
  name: string | null;
  briefing: string | null;
  sharedDir: string | null;
  /**
   * colab_group landed as a separate feature (#54). This PR's dialog doesn't
   * surface a field for it yet, so we always pass null — the Rust side
   * defaults to the session's `--name`, matching the CLI's behavior when
   * `--colab-group` is omitted. A future PR can extend the launch form.
   */
  colabGroup: string | null;
  model: string | null;
  [key: string]: unknown;
}

export function buildLaunchArgs(form: LaunchFormValues): LaunchInvokeArgs {
  return {
    name: cleanFormArg(form.name),
    briefing: cleanFormArg(form.briefing),
    sharedDir: cleanFormArg(form.sharedDir),
    colabGroup: null,
    model: cleanFormArg(form.model),
  };
}

export interface ClaimFormValues {
  name: string;
  force: boolean;
}

export interface ClaimInvokeArgs {
  name: string;
  force: boolean;
  /** See the `LaunchInvokeArgs.colabGroup` note — not surfaced here yet. */
  colabGroup: string | null;
  [key: string]: unknown;
}

/**
 * Build the claim command argument bag. Throws if no session was picked —
 * `null`/empty would silently claim whichever session the twapp host happens
 * to be running in, which is rarely what the user wants.
 */
export function buildClaimArgs(form: ClaimFormValues): ClaimInvokeArgs {
  const picked = form.name.trim();
  if (!picked) {
    throw new Error("Pick a session to claim.");
  }
  return {
    name: picked,
    force: form.force,
    colabGroup: null,
  };
}
