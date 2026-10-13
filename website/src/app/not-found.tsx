import type { Metadata } from "@farm.js/core";
import { SiteFooter, SiteHeader } from "../components/site-chrome";

export const metadata: Metadata = {
  title: "Not found · modebox",
};

export default function NotFound() {
  return (
    <div className="frame">
      <SiteHeader />
      <main className="guide">
        <section className="guide-intro corners">
          <p className="eyebrow">404</p>
          <h1>Nothing at this address.</h1>
          <p className="lede">
            Try the <a className="inline" href="/">home page</a> or the{" "}
            <a className="inline" href="/setup">setup guide</a>.
          </p>
        </section>
      </main>
      <SiteFooter />
    </div>
  );
}
