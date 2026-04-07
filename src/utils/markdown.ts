import { normalizeFilePathCandidate, splitTextWithFilePaths } from "./file";

type MarkdownNode = {
  type: string;
  value?: string;
  url?: string;
  children?: MarkdownNode[];
};

const SKIP_DESCENDANT_TYPES = new Set(["link", "linkReference", "code", "inlineCode", "html"]);

function linkifyTextNode(text: string): MarkdownNode[] {
  return splitTextWithFilePaths(text).map((segment) => {
    if (!segment.href) {
      return { type: "text", value: segment.text };
    }

    return {
      type: "link",
      url: normalizeFilePathCandidate(segment.href),
      children: [{ type: "text", value: segment.text }],
    };
  });
}

function transformNode(node: MarkdownNode) {
  if (!Array.isArray(node.children)) return;

  const nextChildren: MarkdownNode[] = [];
  for (const child of node.children) {
    if (child.type === "text" && typeof child.value === "string") {
      nextChildren.push(...linkifyTextNode(child.value));
      continue;
    }

    if (!SKIP_DESCENDANT_TYPES.has(child.type)) {
      transformNode(child);
    }
    nextChildren.push(child);
  }

  node.children = nextChildren;
}

export function remarkAutolinkFilePaths() {
  return (tree: MarkdownNode) => {
    transformNode(tree);
  };
}
