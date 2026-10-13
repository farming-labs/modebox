import type { LayoutProps, Metadata } from "@farm.js/core";
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./globals.css";

export const metadata: Metadata = {
  title: "modebox: code mode with a pause button",
  description:
    "A WebAssembly sandbox for the code AI agents write. Starts in about 0.1 ms, calls only the tools you hand it, and can pause mid-run for your approval.",
  icons: {
    icon: [{ url: "/favicon.svg", type: "image/svg+xml", sizes: "any" }],
  },
};

export default function RootLayout({ children }: LayoutProps) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <meta name="theme-color" content="#090909" />
      </head>
      <body>{children}</body>
    </html>
  );
}
