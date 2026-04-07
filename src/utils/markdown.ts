import { normalizeFilePathCandidate, splitTextWithFilePaths } from "./file";

type MarkdownNode = {
  type: string;
  value?: string;
  url?: string;
  children?: MarkdownNode[];
};

const SKIP_DESCENDANT_TYPES = new Set(["link", "linkReference", "code", "inlineCode", "html"]);

function linkifyTextNode(text: string, allowAbsolutePaths: boolean): MarkdownNode[] {
  return splitTextWithFilePaths(text, { allowAbsolutePaths }).map((segment) => {
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

function transformNode(node: MarkdownNode, allowAbsolutePaths: boolean) {
  if (!Array.isArray(node.children)) return;

  const nextChildren: MarkdownNode[] = [];
  for (const child of node.children) {
    if (child.type === "text" && typeof child.value === "string") {
      nextChildren.push(...linkifyTextNode(child.value, allowAbsolutePaths));
      continue;
    }

    if (!SKIP_DESCENDANT_TYPES.has(child.type)) {
      transformNode(child, allowAbsolutePaths);
    }
    nextChildren.push(child);
  }

  node.children = nextChildren;
}

export function remarkAutolinkFilePaths(options: { allowAbsolutePaths?: boolean } = {}) {
  const { allowAbsolutePaths = true } = options;
  return (tree: MarkdownNode) => {
    transformNode(tree, allowAbsolutePaths);
  };
}
