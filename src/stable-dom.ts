/** Update dynamic lists without replacing live buttons, details or focus. */
const key = (node: Node): string | null => node instanceof Element
  ? (node.getAttribute('data-render-key') ?? node.id) || null : null;

function patchNode(current: Node, next: Node): void {
  if (current.nodeType === Node.TEXT_NODE) {
    if (current.nodeValue !== next.nodeValue) current.nodeValue = next.nodeValue;
    return;
  }
  if (!(current instanceof Element) || !(next instanceof Element)) return;
  for (const attr of Array.from(current.attributes)) {
    if (!next.hasAttribute(attr.name)) current.removeAttribute(attr.name);
  }
  for (const attr of Array.from(next.attributes)) {
    if (current.getAttribute(attr.name) !== attr.value) current.setAttribute(attr.name, attr.value);
  }
  patchChildren(current, next);
}

function patchChildren(current: Element | DocumentFragment, next: Element | DocumentFragment): void {
  const old = Array.from(current.childNodes);
  const keyed = new Map(old.map(node => [key(node), node]).filter(([id]) => id) as [string, Node][]);
  const used = new Set<Node>();
  let cursor = current.firstChild;
  for (const incoming of Array.from(next.childNodes)) {
    const id = key(incoming);
    let existing = id ? keyed.get(id) : cursor && !key(cursor) ? cursor : null;
    if (existing && (used.has(existing) || existing.nodeType !== incoming.nodeType || existing.nodeName !== incoming.nodeName)) existing = null;
    const node = existing ?? incoming.cloneNode(true);
    if (existing) patchNode(existing, incoming);
    if (node !== cursor) current.insertBefore(node, cursor);
    used.add(node);
    cursor = node.nextSibling;
  }
  for (const node of old) if (!used.has(node)) current.removeChild(node);
}

export function stableMarkup(root: HTMLElement, html: string): void {
  const template = document.createElement('template');
  template.innerHTML = html;
  patchChildren(root, template.content);
}
