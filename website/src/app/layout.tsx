import type { LayoutProps, Metadata } from "@farm.js/core";
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./globals.css";

const TITLE = "modebox: code mode, sandboxed";
const DESCRIPTION =
  "Run the code your AI agent writes in a WebAssembly sandbox that pauses for your approval and picks up later. Nothing runs twice.";

export const metadata: Metadata = {
  metadataBase: "https://modebox.farming-labs.dev",
  title: TITLE,
  description: DESCRIPTION,
  icons: {
    icon: [{ url: "/favicon.svg", type: "image/svg+xml", sizes: "any" }],
    shortcut: "/favicon.ico",
    apple: [{ url: "/apple-touch-icon.png", type: "image/png", sizes: "180x180" }],
  },
  // The image comes from src/app/opengraph-image.png.
  openGraph: { siteName: "modebox", type: "website", url: "/", title: TITLE, description: DESCRIPTION },
  twitter: { card: "summary_large_image", title: TITLE, description: DESCRIPTION },
  other: { "theme-color": "#151515" },
};

// Farm renders the document shell; the root layout wraps the page and adds
// Vercel Web Analytics (cookieless page views, served from this site). The
// script is async, so it never holds up the page.
export default function RootLayout({ children }: LayoutProps) {
  return (
    <>
      <script async src="/_vercel/insights/script.js" />
      {children}
    </>
  );
}
