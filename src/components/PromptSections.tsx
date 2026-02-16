import type { PromptSection, QuickPrompt } from "../types";

export interface EditingPromptState {
  mode: "new-prompt" | "edit-prompt" | "new-section" | "edit-section";
  scope: "global" | "project";
  sectionId: string | null;
  promptId: string | null;
  title: string;
  text: string;
}

interface PromptSectionsProps {
  sections: PromptSection[];
  scope: "global" | "project";
  expandedSections: Set<string>;
  editingPrompt: EditingPromptState | null;
  setEditingPrompt: (state: EditingPromptState | null) => void;
  toggleSection: (key: string) => void;
  savePromptEdit: () => void;
  startEditSection: (scope: "global" | "project", section: PromptSection) => void;
  startNewPrompt: (scope: "global" | "project", sectionId: string) => void;
  startEditPrompt: (scope: "global" | "project", sectionId: string, prompt: QuickPrompt) => void;
  deleteSection: (scope: "global" | "project", sectionId: string) => void;
  deletePrompt: (scope: "global" | "project", sectionId: string, promptId: string) => void;
  sendPrompt: (text: string) => void;
}

export default function PromptSections({
  sections,
  scope,
  expandedSections,
  editingPrompt,
  setEditingPrompt,
  toggleSection,
  savePromptEdit,
  startEditSection,
  startNewPrompt,
  startEditPrompt,
  deleteSection,
  deletePrompt,
  sendPrompt,
}: PromptSectionsProps) {
  return (
    <>
      {sections.map((section) => {
        const sectionKey = `${scope}-${section.id}`;
        const isExpanded = expandedSections.has(sectionKey);
        return (
          <div key={sectionKey} className="prompt-section">
            <div className="prompt-section-header" onClick={() => toggleSection(sectionKey)}>
              <span className={`prompt-chevron ${isExpanded ? "expanded" : ""}`}>&#9654;</span>
              {editingPrompt?.mode === "edit-section" && editingPrompt.sectionId === section.id && editingPrompt.scope === scope ? (
                <input
                  className="prompt-inline-input"
                  value={editingPrompt.title}
                  onChange={(e) => setEditingPrompt({ ...editingPrompt, title: e.target.value })}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") savePromptEdit();
                    if (e.key === "Escape") setEditingPrompt(null);
                  }}
                  onClick={(e) => e.stopPropagation()}
                  autoFocus
                />
              ) : (
                <span className="prompt-section-title">{section.title}</span>
              )}
              <span className={`prompt-scope-badge scope-${scope}`}>{scope === "global" ? "G" : "P"}</span>
              <div className="prompt-section-actions">
                {editingPrompt?.mode === "edit-section" && editingPrompt.sectionId === section.id && editingPrompt.scope === scope ? (
                  <button className="prompt-action-btn" onClick={(e) => { e.stopPropagation(); savePromptEdit(); }} title="Save">&#10003;</button>
                ) : (
                  <button className="prompt-action-btn" onClick={(e) => { e.stopPropagation(); startEditSection(scope, section); }} title="Rename">&#9998;</button>
                )}
                <button className="prompt-action-btn prompt-action-delete" onClick={(e) => { e.stopPropagation(); deleteSection(scope, section.id); }} title="Delete section">&times;</button>
              </div>
            </div>
            {isExpanded && (
              <div className="prompt-section-items">
                {section.prompts.map((prompt) => (
                  <div key={prompt.id} className="prompt-item">
                    {editingPrompt?.mode === "edit-prompt" && editingPrompt.promptId === prompt.id && editingPrompt.scope === scope ? (
                      <div className="prompt-edit-form" onClick={(e) => e.stopPropagation()}>
                        <input
                          placeholder="Title"
                          value={editingPrompt.title}
                          onChange={(e) => setEditingPrompt({ ...editingPrompt, title: e.target.value })}
                          autoFocus
                        />
                        <textarea
                          placeholder="Prompt text..."
                          value={editingPrompt.text}
                          onChange={(e) => {
                            setEditingPrompt({ ...editingPrompt, text: e.target.value });
                            e.target.style.height = "auto";
                            e.target.style.height = e.target.scrollHeight + "px";
                          }}
                          ref={(el) => { if (el) { el.style.height = "auto"; el.style.height = el.scrollHeight + "px"; } }}
                          onKeyDown={(e) => {
                            if (e.key === "Enter" && e.metaKey) savePromptEdit();
                            if (e.key === "Escape") setEditingPrompt(null);
                          }}
                        />
                        <div className="prompt-edit-form-actions">
                          <button className="prompt-form-cancel" onClick={() => setEditingPrompt(null)}>Cancel</button>
                          <button className="prompt-form-save" onClick={savePromptEdit}>Save</button>
                        </div>
                      </div>
                    ) : (
                      <>
                        <span className="prompt-item-title" title={prompt.text} onClick={() => sendPrompt(prompt.text)}>{prompt.title}</span>
                        <div className="prompt-item-actions">
                          <button className="prompt-action-btn" onClick={() => sendPrompt(prompt.text)} title="Send to terminal">&#8629;</button>
                          <button className="prompt-action-btn" onClick={() => startEditPrompt(scope, section.id, prompt)} title="Edit">&#9998;</button>
                          <button className="prompt-action-btn prompt-action-delete" onClick={() => deletePrompt(scope, section.id, prompt.id)} title="Delete">&times;</button>
                        </div>
                      </>
                    )}
                  </div>
                ))}
                {editingPrompt?.mode === "new-prompt" && editingPrompt.sectionId === section.id && editingPrompt.scope === scope ? (
                  <div className="prompt-edit-form">
                    <input
                      placeholder="Title"
                      value={editingPrompt.title}
                      onChange={(e) => setEditingPrompt({ ...editingPrompt, title: e.target.value })}
                      autoFocus
                    />
                    <textarea
                      placeholder="Prompt text..."
                      value={editingPrompt.text}
                      onChange={(e) => {
                        setEditingPrompt({ ...editingPrompt, text: e.target.value });
                        e.target.style.height = "auto";
                        e.target.style.height = e.target.scrollHeight + "px";
                      }}
                      ref={(el) => { if (el) { el.style.height = "auto"; el.style.height = el.scrollHeight + "px"; } }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && e.metaKey) savePromptEdit();
                        if (e.key === "Escape") setEditingPrompt(null);
                      }}
                    />
                    <div className="prompt-edit-form-actions">
                      <button className="prompt-form-cancel" onClick={() => setEditingPrompt(null)}>Cancel</button>
                      <button className="prompt-form-save" onClick={savePromptEdit}>Save</button>
                    </div>
                  </div>
                ) : (
                  <button className="prompt-add-item" onClick={() => startNewPrompt(scope, section.id)}>+ Add prompt</button>
                )}
              </div>
            )}
          </div>
        );
      })}
    </>
  );
}
