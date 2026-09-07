import { AppProviders } from "@/app/providers";
import { AppShell } from "@/app/AppShell";

export default function App() {
  return (
    <AppProviders>
      <AppShell />
    </AppProviders>
  );
}
