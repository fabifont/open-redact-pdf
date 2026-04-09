import { useRef, type ChangeEvent } from "react";
import { ZOOM_LEVELS } from "../App";

type ToolbarProps = {
  onFileChange: (event: ChangeEvent<HTMLInputElement>) => void;
  status: string;
  error: string | null;
  zoom: number;
  onZoomChange: (zoom: number) => void;
};

export function Toolbar({
  onFileChange,
  status,
  error,
  zoom,
  onZoomChange,
}: ToolbarProps) {
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  return (
    <div className="toolbar">
      <span className="toolbar-title">Open Redact PDF</span>
      <button
        type="button"
        className="btn"
        onClick={() => fileInputRef.current?.click()}
      >
        Open PDF
      </button>
      <input
        ref={fileInputRef}
        type="file"
        accept="application/pdf"
        className="file-input-hidden"
        onChange={onFileChange}
      />
      <div className="zoom-controls">
        <button
          type="button"
          className="btn btn-icon"
          disabled={zoom <= ZOOM_LEVELS[0]}
          onClick={() => {
            const previous = ZOOM_LEVELS.filter((level) => level < zoom);
            if (previous.length > 0) onZoomChange(previous[previous.length - 1]);
          }}
        >
          -
        </button>
        <span className="zoom-label">{Math.round(zoom * 100)}%</span>
        <button
          type="button"
          className="btn btn-icon"
          disabled={zoom >= ZOOM_LEVELS[ZOOM_LEVELS.length - 1]}
          onClick={() => {
            const next = ZOOM_LEVELS.filter((level) => level > zoom);
            if (next.length > 0) onZoomChange(next[0]);
          }}
        >
          +
        </button>
      </div>
      <div className="toolbar-spacer" />
      {error ? (
        <span className="toolbar-error">{error}</span>
      ) : (
        <span className="toolbar-status">{status}</span>
      )}
    </div>
  );
}
