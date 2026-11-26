import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { HubApi } from "../types";

export async function dashboard(api: HubApi) {
  render(<App api={api} />);
  await screen.findByRole("heading", { name: /(?:Your setup|Il tuo setup)/ });
}

export async function audioView(api: HubApi) {
  const user = userEvent.setup();
  await dashboard(api);
  await user.click(screen.getByRole("tab", { name: /Audio/ }));
  return { user, eq: within(screen.getByRole("region", { name: "Headphone equalizer" })) };
}
