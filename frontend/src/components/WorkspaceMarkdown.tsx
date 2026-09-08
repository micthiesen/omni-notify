import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

const components: Components = {
  // Artifact cards already have an h3; keep generated headings below that level.
  h1: ({ children }) => <h4>{children}</h4>,
  h2: ({ children }) => <h4>{children}</h4>,
  h3: ({ children }) => <h5>{children}</h5>,
  h4: ({ children }) => <h6>{children}</h6>,
  a: ({ href, children, title }) =>
    href ? (
      <a href={href} title={title} target="_blank" rel="noopener noreferrer">
        {children}
      </a>
    ) : (
      <span>{children}</span>
    ),
  table: ({ children }) => (
    <div
      className="workspace-markdown-table"
      role="region"
      aria-label="Research table"
      tabIndex={0}
    >
      <table>{children}</table>
    </div>
  ),
  img: ({ src, alt, title }) => (
    <img src={src} alt={alt ?? ""} title={title} loading="lazy" />
  ),
};

/** Render generated research as React elements, retaining the library's safe URL transform. */
export function WorkspaceMarkdown({ content }: { content: string }) {
  return (
    <div className="workspace-markdown">
      <Markdown remarkPlugins={[remarkGfm]} components={components} skipHtml>
        {content}
      </Markdown>
    </div>
  );
}
