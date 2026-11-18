/// <reference types="vite/client" />
import { FluentBundle, FluentResource, type FluentVariable } from "@fluent/bundle";
import manifest from "../locales/manifest.json?raw";
import type { AppLocale } from "./types";

type Language = { id: string; name: string; direction: "ltr" | "rtl"; format: string };
type Args = Record<string, FluentVariable>;
type Catalog = { bundle: FluentBundle; resource: FluentResource };

export const languages: readonly Language[] = JSON.parse(manifest);
export const initialLocale: AppLocale = {
  preference: "zh-Hans",
  language: "zh-Hans",
  formatLocale: "zh-CN",
};

const raw = import.meta.glob<string>("../locales/*.ftl", {
  eager: true,
  query: "?raw",
  import: "default",
});
const catalogs = new Map<string, Catalog>();
let active = initialLocale;

function catalog(language: string) {
  const cached = catalogs.get(language);
  if (cached) return cached;
  const source = raw[`../locales/${language}.ftl`];
  if (source == null) return;
  const bundle = new FluentBundle(language);
  const resource = new FluentResource(source);
  if (bundle.addResource(resource).length) return;
  const result = { bundle, resource };
  catalogs.set(language, result);
  return result;
}

export function message(id: string, args?: Args): string | undefined {
  for (const language of new Set([active.language, "en", initialLocale.language])) {
    const bundle = catalog(language)?.bundle;
    const pattern = bundle?.getMessage(id)?.value;
    if (bundle && pattern != null) {
      const errors: Error[] = [];
      const value = bundle.formatPattern(pattern, args, errors);
      if (!errors.length) return value;
    }
  }
}

const literals = new Map<string, string>();
const templates = new Map<string, { names: string[]; pattern: RegExp; specificity: number }>();
let sourceIndexed = false;

function escapeRegExp(text: string) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// The source-language patterns provide a bridge for existing application text
// at explicitly marked display sites. Interpolated values remain opaque.
function indexSource() {
  if (sourceIndexed) return;
  sourceIndexed = true;
  for (const { id, value } of catalog(initialLocale.language)?.resource.body ?? []) {
    if (!id.startsWith("ui-") || value == null) continue;
    if (typeof value === "string") {
      literals.set(value, id);
      continue;
    }
    const parts: string[] = [];
    const names: string[] = [];
    let specificity = 0;
    for (const part of value) {
      if (typeof part === "string" || part.type === "str") {
        const text = typeof part === "string" ? part : part.value;
        specificity += text.length;
        parts.push(escapeRegExp(text));
      } else if (part.type === "var") {
        names.push(part.name);
        parts.push("(.*?)");
      }
    }
    if (!names.length) literals.set(parts.join("").replace(/\\(.)/g, "$1"), id);
    else if (specificity) {
      templates.set(id, { names, specificity, pattern: new RegExp(`^${parts.join("")}$`, "su") });
    }
  }
  const sorted = [...templates].sort((a, b) => b[1].specificity - a[1].specificity);
  templates.clear();
  for (const [id, pattern] of sorted) templates.set(id, pattern);
}

const captured = new Map<string, { id: string; args?: Args } | null>();
function match(source: string, id?: string) {
  const known = id ? undefined : captured.get(source);
  if (known !== undefined) return known;
  const exact = id ?? literals.get(source);
  const candidates = exact ? [exact] : templates.keys();
  let result: { id: string; args?: Args } | null = null;
  for (const candidate of candidates) {
    const template = templates.get(candidate);
    if (!template) {
      result = { id: candidate };
      break;
    }
    const matches = template.pattern.exec(source);
    if (matches) {
      result = { id: candidate, args: Object.fromEntries(template.names.map((name, index) => [name, matches[index + 1]])) };
      break;
    }
  }
  if (!id && source.length < 1500) {
    if (captured.size >= 2048) captured.clear();
    captured.set(source, result);
  }
  return result;
}

export function localizeText(source: string, id?: string) {
  if (active.language === initialLocale.language || !source.trim()) return source;
  const text = source.trim();
  if (!id && !/\p{Script=Han}/u.test(text)) return source;
  indexSource();
  const found = match(text, id);
  if (!found) return source;
  const translated = message(found.id, found.args);
  return translated == null ? source : source.replace(text, () => translated);
}

type Binding = { source: string; id?: string; rendered?: string };
type MessageArgs = Args | (() => Args);
const textBindings = new Map<Node, Binding>();
const dateBindings = new Map<Node, () => string>();
const messageBindings = new Map<Node, { id: string; fallback: string; args?: MessageArgs }>();
const attributeBindings = new Map<Element, Map<string, Binding>>();
const attributes = ["title", "aria-label", "aria-description", "placeholder", "alt"] as const;

function paintText(node: Node, binding: Binding) {
  const rendered = localizeText(binding.source, binding.id);
  binding.rendered = rendered;
  if (node.textContent !== rendered) node.textContent = rendered;
}

export function setLocalizedText<T extends Node>(node: T, source: string | null | undefined) {
  const value = source ?? "";
  messageBindings.delete(node);
  dateBindings.delete(node);
  if (/[\u3400-\u9fff]/u.test(value)) {
    const binding = { source: value };
    textBindings.set(node, binding);
    paintText(node, binding);
  } else {
    textBindings.delete(node);
    if (node.textContent !== value) node.textContent = value;
  }
}

export function setPlainText(node: Node, value: string | null | undefined, fallback: string) {
  if (value == null) {
    setLocalizedText(node, fallback);
  } else {
    textBindings.delete(node);
    messageBindings.delete(node);
    dateBindings.delete(node);
    if (node.textContent !== value) node.textContent = value;
  }
}

export function setLocalizedAttribute(node: Element, name: string, source: string) {
  const bindings = attributeBindings.get(node) ?? new Map<string, Binding>();
  bindings.set(name, { source });
  attributeBindings.set(node, bindings);
  node.setAttribute(name, localizeText(source));
}

export function setMessage(node: Node, id: string, fallback: string, args?: MessageArgs) {
  textBindings.delete(node);
  dateBindings.delete(node);
  messageBindings.set(node, { id, fallback, args });
  node.textContent = message(id, typeof args === "function" ? args() : args) ?? fallback;
}

export function setLocalizedDate(node: Node, format: () => string) {
  textBindings.delete(node);
  messageBindings.delete(node);
  dateBindings.set(node, format);
  node.textContent = format();
}

export function sourceText(node: Node) {
  return textBindings.get(node)?.source ?? node.textContent ?? "";
}

function captureLiveText(node: Element) {
  if (!node.hasAttribute("data-i18n-live")) return;
  const source = node.textContent ?? "";
  if (textBindings.get(node)?.rendered === source) return;
  const binding = { source };
  textBindings.set(node, binding);
  paintText(node, binding);
}

function captureMarkedText(marker: Comment) {
  const node = marker.nextSibling;
  if (!marker.data.startsWith("i18n:") || !(node instanceof Text)) return;
  if (textBindings.get(node)?.rendered === node.data) return;
  const binding = { source: node.data, id: marker.data.slice(5) || undefined };
  textBindings.set(node, binding);
  paintText(node, binding);
}

function scan(root: Node) {
  if (root.nodeType !== Node.ELEMENT_NODE && root.nodeType !== Node.COMMENT_NODE) return;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_COMMENT | NodeFilter.SHOW_ELEMENT);
  do {
    const node = walker.currentNode;
    if (node instanceof Comment) {
      captureMarkedText(node);
    } else if (node instanceof Element && !attributeBindings.has(node)) {
      captureLiveText(node);
      const id = node.getAttribute("data-i18n-text");
      if (id && node.children.length === 0 && !textBindings.has(node)) {
        const binding = { source: node.textContent ?? "", id };
        textBindings.set(node, binding);
        paintText(node, binding);
      }
      const bindings = new Map<string, Binding>();
      for (const name of attributes) {
        const id = node.getAttribute(`data-i18n-${name}`);
        const source = node.getAttribute(name);
        if (id && source != null) {
          const binding = { id, source };
          bindings.set(name, binding);
          node.setAttribute(name, localizeText(source, id));
        }
      }
      if (bindings.size) attributeBindings.set(node, bindings);
    }
  } while (walker.nextNode());
}

function pruneBindings() {
  for (const node of textBindings.keys()) {
    if (!node.isConnected) textBindings.delete(node);
  }
  for (const node of messageBindings.keys()) {
    if (!node.isConnected) messageBindings.delete(node);
  }
  for (const node of dateBindings.keys()) {
    if (!node.isConnected) dateBindings.delete(node);
  }
  for (const node of attributeBindings.keys()) {
    if (!node.isConnected) attributeBindings.delete(node);
  }
}

export function observeLocalizedMarkup(root: Element) {
  scan(root);
  const observer = new MutationObserver(records => {
    let removed = false;
    for (const record of records) {
      if (record.target instanceof Text) {
        if (record.target.previousSibling instanceof Comment) captureMarkedText(record.target.previousSibling);
        if (record.target.parentElement) captureLiveText(record.target.parentElement);
      } else if (record.target instanceof Element) captureLiveText(record.target);
      if (record.removedNodes.length) removed = true;
      for (const node of record.addedNodes) scan(node);
    }
    if (removed) pruneBindings();
  });
  observer.observe(root, { subtree: true, childList: true, characterData: true });
  return () => observer.disconnect();
}

export function applyAppLocale(locale: AppLocale) {
  const language = languages.find(language => language.id === locale.language);
  if (!language || !catalog(language.id)) throw new Error("Invalid application locale");
  active = locale;
  document.documentElement.lang = locale.language;
  document.documentElement.dir = language.direction;
  pruneBindings();
  for (const [node, binding] of textBindings) paintText(node, binding);
  for (const [node, { id, fallback, args }] of messageBindings) {
    node.textContent = message(id, typeof args === "function" ? args() : args) ?? fallback;
  }
  for (const [node, format] of dateBindings) node.textContent = format();
  for (const [node, bindings] of attributeBindings) {
    for (const [name, binding] of bindings) node.setAttribute(name, localizeText(binding.source, binding.id));
  }
}

export function formatLocale() {
  return active.formatLocale;
}

export function localeOptions() {
  return languages.map(({id, name, direction}) =>
    `<option value="${id}" lang="${id}" dir="${direction}">${name}</option>`
  ).join("");
}
