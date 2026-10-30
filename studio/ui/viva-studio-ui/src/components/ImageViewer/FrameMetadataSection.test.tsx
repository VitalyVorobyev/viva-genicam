import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { FrameMetadataSection } from "./FrameMetadataSection";

const metadata = {
  block_id: "18446744073709551615", ts_dev: "18446744073709551614", ts_host_ns: null,
  complete: true, chunks: [{ id: "42", bytes: 8 }],
  features: [{ name: "ChunkGain", value: null, error: "Not attached" }],
  chunk_error: "Invalid trailer", truncated: true,
};

it("shows exact device ticks, uncalibrated host time and chunk errors as frame facts", () => {
  render(<FrameMetadataSection metadata={metadata} frozen />);
  expect(screen.getByText("18446744073709551615")).toBeTruthy();
  expect(screen.getByText("18446744073709551614")).toBeTruthy();
  expect(screen.getByText("Not calibrated")).toBeTruthy();
  expect(screen.getByText("Frozen frame")).toBeTruthy();
  expect(screen.getByText("ID 42")).toBeTruthy();
  expect(screen.getByText("8 bytes")).toBeTruthy();
  expect(screen.getByText("Error: Not attached")).toBeTruthy();
  expect(screen.getByText("Chunk data: Invalid trailer")).toBeTruthy();
  expect(screen.getByText(/Some frame metadata exceeds/)).toBeTruthy();
  expect(screen.queryByRole("button")).toBeNull();
});

it("does not carry the previous frame's chunks into a plain frame", () => {
  const { rerender } = render(<FrameMetadataSection metadata={metadata} />);
  rerender(<FrameMetadataSection metadata={{ ...metadata, chunks: [], features: [], chunk_error: null, truncated: false }} />);
  expect(screen.queryByText("ChunkGain")).toBeNull();
  expect(screen.getByText("No chunk data in this frame.")).toBeTruthy();
  rerender(<FrameMetadataSection metadata={null} />);
  expect(screen.queryByText("18446744073709551615")).toBeNull();
});
