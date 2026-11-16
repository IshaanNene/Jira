/** What went wrong on the way back from a workspace's identity provider (`?sso_error=`). */
export function ssoErrorMessage(code: string | null): string | null {
  switch (code) {
    case null:
    case "":
      return null;
    case "not_linked":
      return "Your account isn't linked to this workspace's single sign-on yet. Sign in with your password or passkey, then link it under Settings → Security.";
    case "already_linked":
      return "That identity is already linked to another Meridian account.";
    case "not_member":
      return "You're not a member of that workspace.";
    case "cancelled":
      return "Single sign-on was cancelled.";
    case "expired":
      return "That sign-in attempt expired or was opened in another browser. Try again.";
    case "suspended":
      return "This account can't sign in right now.";
    case "provider":
      return "The identity provider's answer couldn't be verified. Try again, or ask your workspace admin to check the set-up.";
    default:
      return "Single sign-on isn't available right now. Try again in a moment.";
  }
}
