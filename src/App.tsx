import { useAppStateInit } from "@/hooks/use-app-state";
import { AppShell } from "@/components/layout/app-shell";

function App() {
  useAppStateInit();
  return <AppShell />;
}

export default App;
