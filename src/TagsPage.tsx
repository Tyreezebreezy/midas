import { useEffect, useState } from "react";
import type { Tag } from "./api/midas";
import { tagsApi } from "./api/midas";

function TagsPage() {
  const [tags, setTags] = useState<Tag[]>([]);
  const [newName, setNewName] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      setTags(await tagsApi.list());
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
  }, []);

  async function handleCreate(e: React.FormEvent) {
    e.preventDefault();
    if (newName.trim() === "") return;
    setSaving(true);
    setError(null);
    try {
      await tagsApi.create(newName.trim());
      setNewName("");
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function handleDelete(tag: Tag) {
    setError(null);
    try {
      await tagsApi.delete(tag.id);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <section aria-labelledby="tags-heading">
      <h3 id="tags-heading">Tags</h3>
      <p className="muted">
        Tags are lightweight labels you can attach to any transaction — use them
        for things that cut across categories, like a trip or a project.
      </p>

      <form className="inline-form" onSubmit={handleCreate}>
        <label htmlFor="new-tag-name" className="sr-only">
          New tag name
        </label>
        <input
          id="new-tag-name"
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          placeholder="New tag name"
        />
        <button type="submit" className="btn-primary" disabled={saving}>
          Add tag
        </button>
      </form>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="muted">Loading…</p>
      ) : tags.length === 0 ? (
        <p className="muted">No tags yet.</p>
      ) : (
        <ul className="tag-list">
          {tags.map((tag) => (
            <li key={tag.id} className="tag-list-item">
              <span>{tag.name}</span>
              <button
                type="button"
                className="btn-link btn-link-danger"
                onClick={() => handleDelete(tag)}
              >
                Delete
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

export default TagsPage;
