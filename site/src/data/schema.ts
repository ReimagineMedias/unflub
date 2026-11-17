import { SITE } from "./site";
import { FAQ } from "./faq";

const abs = (path: string): string => new URL(path, SITE.url).href;

export const softwareApplication = {
  "@context": "https://schema.org",
  "@type": "SoftwareApplication",
  name: "unflub",
  description: "Free, open-source, offline, phone-style autocorrect for every text field on your computer.",
  url: abs("/"),
  image: abs(SITE.ogImage),
  operatingSystem: "Windows",
  applicationCategory: "UtilitiesApplication",
  softwareVersion: SITE.version,
  downloadUrl: SITE.download,
  license: "https://opensource.org/licenses/MIT",
  isAccessibleForFree: true,
  offers: { "@type": "Offer", price: "0", priceCurrency: "USD" },
  publisher: { "@id": `${SITE.org.url}/#organization` },
};

export const website = {
  "@context": "https://schema.org",
  "@type": "WebSite",
  name: "unflub",
  url: abs("/"),
  publisher: { "@id": `${SITE.org.url}/#organization` },
};

/** FAQPage JSON-LD for a list of plain-text questions and answers shown on the same page. */
export const faqSchema = (items: readonly { q: string; a: string }[]) => ({
  "@context": "https://schema.org",
  "@type": "FAQPage",
  mainEntity: items.map((f) => ({
    "@type": "Question",
    name: f.q,
    acceptedAnswer: { "@type": "Answer", text: f.a },
  })),
});

export const faqPage = faqSchema(FAQ);
