import { Fragment, type ReactNode } from "react";
import { ExternalLink } from "./components";
import { t } from "./i18n";
import { isExternalUrl } from "./links";

const COMMIT_SUFFIX = /\s*\(\[[^\]\n]+\]\(https:\/\/github\.com\/ReimagineMedias\/unflub\/commit\/[a-f\d]+\/?\)\)/gi;
const INLINE = /\[([^\]\n]+)\]\(([^)\s]+)\)|\*\*([^*\n]+)\*\*|[*_]([^*_\n]+)[*_]|`([^`\n]+)`/g;

function inline(text: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  let end = 0;
  for (const match of text.matchAll(INLINE)) {
    const { index } = match;
    const [token, label, url, bold, italic, code] = match;
    nodes.push(text.slice(end, index));
    if (bold) nodes.push(<strong key={index}>{bold}</strong>);
    else if (italic) nodes.push(<em key={index}>{italic}</em>);
    else if (code) nodes.push(<code key={index}>{code}</code>);
    else if (isExternalUrl(url)) nodes.push(<ExternalLink key={index} href={url}>{label}</ExternalLink>);
    else nodes.push(label);
    end = index + token.length;
  }
  nodes.push(text.slice(end));
  return nodes;
}

const HEADING = /^(#{1,6})\s+(.+)$/;
const BULLET = /^[-*+]\s+(.+)$/;
const NUMBERED = /^\d+[.)]\s+(.+)$/;

function blocks(notes: string) {
  const lines = notes.replace(COMMIT_SUFFIX, "").trim().split(/\r?\n/).map((line) => line.trim());
  const content: ReactNode[] = [];
  for (let i = 0; i < lines.length;) {
    const start = i;
    const line = lines[i++];
    if (!line) continue;
    const heading = HEADING.exec(line);
    if (heading) {
      const Heading = heading[1].length <= 2 ? "h3" : "h4";
      content.push(<Heading key={start}>{inline(heading[2])}</Heading>);
      continue;
    }
    const pattern = BULLET.test(line) ? BULLET : NUMBERED.test(line) ? NUMBERED : null;
    if (pattern) {
      const List = pattern === BULLET ? "ul" : "ol";
      const items = [line.replace(pattern, "$1")];
      while (i < lines.length && pattern.test(lines[i])) items.push(lines[i++].replace(pattern, "$1"));
      content.push(<List key={start}>{items.map((item, index) => <li key={index}>{inline(item)}</li>)}</List>);
      continue;
    }
    const paragraph = [line];
    while (lines[i] && ![HEADING, BULLET, NUMBERED].some((pattern) => pattern.test(lines[i]))) {
      paragraph.push(lines[i++]);
    }
    content.push(<p key={start}>{inline(paragraph.join(" "))}</p>);
  }
  return content;
}

/** Release notes are text with a small Markdown subset. React escapes everything else. */
export function ReleaseNotes({ notes }: { notes: string }) {
  const content = blocks(notes);
  if (!content.length) return null;
  return (
    <Fragment>
      <small className="notes-title">{t("about.notes")}</small>
      <div className="notes" role="region" aria-label={t("about.notes")} tabIndex={0}>{content}</div>
    </Fragment>
  );
}
