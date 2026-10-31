import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/** Configures the deterministic production bundle embedded by the Rust binary. */
export default defineConfig({ plugins: [react()] });
