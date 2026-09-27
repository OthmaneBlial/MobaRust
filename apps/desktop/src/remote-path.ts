export function remoteParentPath(path: string): string {
  const withoutTrailingSlashes = path.replace(/\/+$/, "") || "/";
  if (withoutTrailingSlashes === ".") return "..";
  if (withoutTrailingSlashes === "/") return "/";
  if (withoutTrailingSlashes === ".." || (!withoutTrailingSlashes.startsWith("/") && withoutTrailingSlashes.endsWith("/.."))) {
    return `${withoutTrailingSlashes}/..`;
  }
  const separator = withoutTrailingSlashes.lastIndexOf("/");
  return separator < 0 ? "." : separator === 0 ? "/" : withoutTrailingSlashes.slice(0, separator);
}

export function remoteChildPath(parent: string, name: string): string {
  if (parent === ".") return `./${name}`;
  if (parent === "/") return `/${name}`;
  return `${parent.replace(/\/+$/, "")}/${name}`;
}
