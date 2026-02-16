import React from "react";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function renderJsonNode(
  value: any,
  path: string,
  depth: number,
  jsonCollapsed: Set<string>,
  toggleJsonCollapse: (path: string) => void
): React.ReactNode {
  if (value === null) return <span className="json-null">null</span>;
  if (typeof value === "boolean") return <span className="json-boolean">{String(value)}</span>;
  if (typeof value === "number") return <span className="json-number">{value}</span>;
  if (typeof value === "string") return <span className="json-string">&quot;{value}&quot;</span>;

  if (Array.isArray(value)) {
    if (value.length === 0) return <span className="json-bracket">[]</span>;
    const collapsed = jsonCollapsed.has(path);
    return (
      <span>
        <span className="json-collapse-toggle" onClick={() => toggleJsonCollapse(path)}>
          <span className={`prompt-chevron${collapsed ? "" : " expanded"}`}>&#9654;</span>
        </span>
        <span className="json-bracket">[</span>
        {collapsed ? (
          <span className="json-collapsed-indicator" onClick={() => toggleJsonCollapse(path)}>
            {value.length} {value.length === 1 ? "item" : "items"}
          </span>
        ) : (
          <div className="json-children">
            {value.map((item, i) => (
              <div key={i} className="json-entry" style={{ paddingLeft: `${(depth + 1) * 16}px` }}>
                {renderJsonNode(item, `${path}[${i}]`, depth + 1, jsonCollapsed, toggleJsonCollapse)}
                {i < value.length - 1 && <span className="json-comma">,</span>}
              </div>
            ))}
          </div>
        )}
        {!collapsed && <div style={{ paddingLeft: `${depth * 16}px` }}><span className="json-bracket">]</span></div>}
        {collapsed && <span className="json-bracket">]</span>}
      </span>
    );
  }

  if (typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.length === 0) return <span className="json-bracket">{"{}"}</span>;
    const collapsed = jsonCollapsed.has(path);
    return (
      <span>
        <span className="json-collapse-toggle" onClick={() => toggleJsonCollapse(path)}>
          <span className={`prompt-chevron${collapsed ? "" : " expanded"}`}>&#9654;</span>
        </span>
        <span className="json-bracket">{"{"}</span>
        {collapsed ? (
          <span className="json-collapsed-indicator" onClick={() => toggleJsonCollapse(path)}>
            {entries.length} {entries.length === 1 ? "key" : "keys"}
          </span>
        ) : (
          <div className="json-children">
            {entries.map(([key, val], i) => (
              <div key={key} className="json-entry" style={{ paddingLeft: `${(depth + 1) * 16}px` }}>
                <span className="json-key">&quot;{key}&quot;</span>
                <span className="json-colon">: </span>
                {renderJsonNode(val, `${path}.${key}`, depth + 1, jsonCollapsed, toggleJsonCollapse)}
                {i < entries.length - 1 && <span className="json-comma">,</span>}
              </div>
            ))}
          </div>
        )}
        {!collapsed && <div style={{ paddingLeft: `${depth * 16}px` }}><span className="json-bracket">{"}"}</span></div>}
        {collapsed && <span className="json-bracket">{"}"}</span>}
      </span>
    );
  }

  return <span>{String(value)}</span>;
}

export function renderYamlNode(
  value: unknown,
  path: string,
  depth: number,
  jsonCollapsed: Set<string>,
  toggleJsonCollapse: (path: string) => void
): React.ReactNode {
  if (value === null) return <span className="json-null">null</span>;
  if (typeof value === "boolean") return <span className="json-boolean">{String(value)}</span>;
  if (typeof value === "number") return <span className="json-number">{value}</span>;
  if (typeof value === "string") return <span className="json-string">{value}</span>;

  if (Array.isArray(value)) {
    if (value.length === 0) return <span className="json-bracket">[]</span>;
    const collapsed = jsonCollapsed.has(path);
    return (
      <span>
        <span className="json-collapse-toggle" onClick={() => toggleJsonCollapse(path)}>
          <span className={`prompt-chevron${collapsed ? "" : " expanded"}`}>&#9654;</span>
        </span>
        {collapsed ? (
          <span className="json-collapsed-indicator" onClick={() => toggleJsonCollapse(path)}>
            {value.length} {value.length === 1 ? "item" : "items"}
          </span>
        ) : (
          <div className="json-children">
            {value.map((item, i) => {
              const isComplex = item !== null && typeof item === "object";
              return (
                <div key={i} className="json-entry" style={{ paddingLeft: `${(depth + 1) * 16}px` }}>
                  <span className="yaml-dash">- </span>
                  {isComplex ? renderYamlNode(item, `${path}[${i}]`, depth + 1, jsonCollapsed, toggleJsonCollapse) : renderYamlNode(item, `${path}[${i}]`, depth + 1, jsonCollapsed, toggleJsonCollapse)}
                </div>
              );
            })}
          </div>
        )}
      </span>
    );
  }

  if (typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.length === 0) return <span className="json-bracket">{"{}"}</span>;
    const collapsed = jsonCollapsed.has(path);
    return (
      <span>
        {depth > 0 && (
          <span className="json-collapse-toggle" onClick={() => toggleJsonCollapse(path)}>
            <span className={`prompt-chevron${collapsed ? "" : " expanded"}`}>&#9654;</span>
          </span>
        )}
        {collapsed ? (
          <span className="json-collapsed-indicator" onClick={() => toggleJsonCollapse(path)}>
            {entries.length} {entries.length === 1 ? "key" : "keys"}
          </span>
        ) : (
          <div className={depth === 0 ? "json-children" : "json-children"}>
            {entries.map(([key, val]) => {
              const isComplex = val !== null && typeof val === "object";
              return (
                <div key={key} className="json-entry" style={{ paddingLeft: `${(depth > 0 ? (depth + 1) : 0) * 16}px` }}>
                  <span className="json-key">{key}</span>
                  <span className="json-colon">:</span>
                  {isComplex ? (
                    <>{" "}{renderYamlNode(val, `${path}.${key}`, depth + 1, jsonCollapsed, toggleJsonCollapse)}</>
                  ) : (
                    <> <span>{renderYamlNode(val, `${path}.${key}`, depth + 1, jsonCollapsed, toggleJsonCollapse)}</span></>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </span>
    );
  }

  return <span>{String(value)}</span>;
}
