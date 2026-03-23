import { Button } from "@/components/ui/button";

function App() {
  return (
    <div className="flex h-screen items-center justify-center bg-background text-foreground">
      <div className="text-center space-y-4">
        <h1 className="text-4xl font-bold">Codemux</h1>
        <p className="text-muted-foreground">React + Tailwind + shadcn scaffold</p>
        <Button variant="outline">Ready</Button>
      </div>
    </div>
  );
}

export default App;
