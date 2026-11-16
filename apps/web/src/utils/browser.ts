/** Leave the app for another site (an identity provider). Separate so tests can observe it. */
export function goTo(url: string): void {
  window.location.assign(url);
}
