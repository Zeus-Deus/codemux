import { useAppStateInit } from "@/hooks/use-app-state";
import { useKeyboardShortcuts } from "@/hooks/use-keyboard-shortcuts";
import { AppShell } from "@/components/layout/app-shell";

function App() {
  useAppStateInit();
  useKeyboardShortcuts();
  return <AppShell />;
}

export default App;
