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

export const faqPage = {
  "@context": "https://schema.org",
  "@type": "FAQPage",
  mainEntity: FAQ.map((f) => ({
    "@type": "Question",
    name: f.q,
    acceptedAnswer: { "@type": "Answer", text: f.a },
  })),
};
