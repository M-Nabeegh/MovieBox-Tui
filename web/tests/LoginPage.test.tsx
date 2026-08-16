import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../src/App";

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

test("submits the password and enters the authenticated shell", async () => {
  let sessionCalls = 0;
  const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const url = String(input);
    if (url.endsWith("/auth/session")) {
      sessionCalls += 1;
      if (sessionCalls === 1) return new Response(null, { status: 401 });
      return json({ authenticated: true, username: "admin", display_name: "Nabeegh", csrf_token: "csrf-fixture" });
    }
    if (url.includes("/discover")) return json([]);
    if (url.includes("/jobs")) return json([]);
    expect(init?.credentials).toBe("same-origin");
    return new Response(null, { status: 204 });
  });

  render(<App />);
  await userEvent.type(await screen.findByLabelText(/server password/i), "fixture-secret");
  await userEvent.click(screen.getByRole("button", { name: /sign in/i }));

  // The browse chrome is what proves the authenticated shell rendered.
  expect(await screen.findByLabelText(/^search$/i)).toBeVisible();
  fetchMock.mockRestore();
});

test("a rejected password is reported without entering the shell", async () => {
  const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    const url = String(input);
    if (url.endsWith("/auth/session")) return new Response(null, { status: 401 });
    return json({ error: { code: "invalid_credentials", message: "That password is not right." } }, 401);
  });

  render(<App />);
  await userEvent.type(await screen.findByLabelText(/server password/i), "wrong");
  await userEvent.click(screen.getByRole("button", { name: /sign in/i }));

  expect(await screen.findByRole("alert")).toHaveTextContent(/not right/i);
  expect(screen.queryByLabelText(/^search$/i)).toBeNull();
  fetchMock.mockRestore();
});
