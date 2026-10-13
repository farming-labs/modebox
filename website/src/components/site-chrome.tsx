import { GitHubIcon } from "./github-icon";

export const REPO = "https://github.com/farming-labs/modebox";

function Logo() {
  return (
    <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
      <rect x="0.5" y="0.5" width="17" height="17" rx="3" fill="none" stroke="currentColor" />
      <rect x="6" y="5" width="2" height="8" fill="currentColor" />
      <rect x="10" y="5" width="2" height="8" fill="currentColor" />
    </svg>
  );
}

export function SiteHeader({ current }: { current?: "setup" }) {
  return (
    <header className="top">
      <a className="brand" href="/">
        <Logo />
        <span>modebox</span>
      </a>
      <nav>
        <a href="/setup" aria-current={current === "setup" ? "page" : undefined}>
          Setup guide
        </a>
        <a href={REPO} className="gh">
          <GitHubIcon />
          GitHub
        </a>
      </nav>
    </header>
  );
}

export function SiteFooter() {
  return (
    <footer className="bottom">
      <span>end of snapshot · MIT · farming-labs</span>
      <span className="bottom-links">
        Built with <a href="https://farmjs.dev">Farm.js</a> ·{" "}
        <a href={REPO} className="gh">
          <GitHubIcon size={12} />
          GitHub
        </a>
      </span>
    </footer>
  );
}
