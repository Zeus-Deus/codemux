// Share one interaction fence across chat hydration, pane selection and
// workspace activation. A late response may hydrate data but cannot navigate.
let sequence = 0;

export function beginRemoteNavigation(): number {
  return ++sequence;
}

export function isCurrentRemoteNavigation(candidate: number): boolean {
  return candidate === sequence;
}
