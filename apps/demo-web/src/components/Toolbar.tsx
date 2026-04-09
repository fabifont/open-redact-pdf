import { useRef, type ChangeEvent } from "react";

type ToolbarProps = {
  onFileChange: (event: ChangeEvent<HTMLInputElement>) => void;
  status: string;
  error: string | null;
};

export function Toolbar({ onFileChange, status, error }: ToolbarProps) {
  const fileRef = useRef<HTMLInputElement | null>(null);

  return (
    <div className="toolbar">
      <span className="toolbar-title">Open Redact PDF</span>
      <button
        type="button"
        className="btn"
        onClick={() => fileRef.current?.click()}
      >
        Open PDF
      </button>
      <input
        ref={fileRef}
        type="file"
        accept="application/pdf"
        className="file-input-hidden"
        onChange={onFileChange}
      />
      <div className="toolbar-spacer" />
      {error ? (
        <span className="toolbar-error">{error}</span>
      ) : (
        <span className="toolbar-status">{status}</span>
      )}
    </div>
  );
}
