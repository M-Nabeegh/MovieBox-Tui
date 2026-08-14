import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../src/App";

test("submits the password and enters the authenticated shell", async () => {
  let sessionCalls = 0;
  const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const url = String(input);
    if (url.endsWith("/auth/session")) {
      sessionCalls += 1;
      if (sessionCalls === 1) return new Response(null, { status: 401 });
      return new Response(JSON.stringify({ authenticated: true, username: "admin", csrf_token: "csrf-fixture" }), { status: 200 });
    }
    expect(init?.credentials).toBe("same-origin");
    return new Response(null, { status: 204 });
  });
  render(<App />);
  await userEvent.type(await screen.findByLabelText(/password/i), "fixture-secret");
  await userEvent.click(screen.getByRole("button", { name: /sign in/i }));
  expect(await screen.findByRole("search")).toBeVisible();
  fetchMock.mockRestore();
});
