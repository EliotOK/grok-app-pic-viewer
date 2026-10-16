/** @vitest-environment jsdom */
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { ProviderErrorDetails } from "./ProviderErrorDetails";

afterEach(cleanup);

it("lets the user open and close the provider's diagnostic response", () => {
  const detail = "HTTP 403: permission-denied";
  render(<ProviderErrorDetails detail={detail} locale="zh" />);
  const button = screen.getByRole("button", { name: "详情" });
  expect(button.getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByText(detail)).toBeNull();
  fireEvent.click(button);
  expect(screen.getByText(detail)).toBeTruthy();
  expect(button.getAttribute("aria-expanded")).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "收起详情" }));
  expect(screen.queryByText(detail)).toBeNull();
});
