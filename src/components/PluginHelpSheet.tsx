/**
 * "How to create a plugin" sheet, opened from the book icon beside the panel
 * title.
 *
 * Short on purpose: the full reference is a manual the agent reads on demand
 * (`<workspace>/.data/manuals/create-plugin.md`, seeded from `prompt.rs`), and
 * this window is the human version — folder shape, manifest, the two protocol
 * directions, service attach and the settings schema. Nothing here talks to
 * the backend.
 */
import { useEffect } from "react";
import { Portal } from "./Portal";
import { FontAwesomeIcon, faXmark } from "../icons";
import { useI18n } from "../i18n";

interface Props {
    onClose: () => void;
}

const FOLDER = `./.data/plugins/<id>/plugin.json
./.data/plugins/<id>/server.mjs
./.data/plugins/<id>/config.schema.yaml`;

const MANIFEST = `{
  "name": "Clock",
  "runtime": "node",
  "entry": "server.mjs",
  "instances": 2,
  "tools": [{ "name": "now", "description": "Current time." }]
}`;

const PROTOCOL = `→ {"type":"hello","plugin":"clock","instance":0}
← {"type":"ready","version":"1.0.0"}
→ {"type":"request","id":"r1","tool":"now","args":{}}
← {"type":"reply","id":"r1","ok":true,"result":"12:00"}`;

const SERVICE = `{"type":"hello","plugin":"clock","token":"…"}`;

const SCHEMA = `sections:
  - key: general
    title: General
    fields:
      - key: greeting
        type: text
        default: "echo:"
      - key: token
        type: secret`;

export function PluginHelpSheet({ onClose }: Props) {
    const { t } = useI18n();

    useEffect(() => {
        const onKeyDown = (event: KeyboardEvent) => {
            if (event.key === "Escape") onClose();
        };
        window.addEventListener("keydown", onKeyDown);
        return () => window.removeEventListener("keydown", onKeyDown);
    }, [onClose]);

    return (
        <Portal>
            <div className="pm-sheet-backdrop" role="presentation" onClick={onClose}>
                <div
                    className="pm-sheet"
                    role="dialog"
                    aria-modal="true"
                    aria-label={t("plugins.help.title")}
                    onClick={(event) => event.stopPropagation()}
                >
                    <header className="pm-sheet-head">
                        <h2>{t("plugins.help.title")}</h2>
                        <button
                            type="button"
                            className="pm-icon-button"
                            onClick={onClose}
                            aria-label={t("plugins.help.close")}
                        >
                            <FontAwesomeIcon icon={faXmark} />
                        </button>
                    </header>

                    <div className="pm-sheet-body">
                        <p className="pm-sheet-intro">{t("plugins.help.intro")}</p>

                        <section className="pm-sheet-section">
                            <h3>{t("plugins.help.folder")}</h3>
                            <p>{t("plugins.help.folderBody")}</p>
                            <pre className="pm-sheet-code">{FOLDER}</pre>
                        </section>

                        <section className="pm-sheet-section">
                            <h3>{t("plugins.help.manifest")}</h3>
                            <p>{t("plugins.help.manifestBody")}</p>
                            <pre className="pm-sheet-code">{MANIFEST}</pre>
                        </section>

                        <section className="pm-sheet-section">
                            <h3>{t("plugins.help.protocol")}</h3>
                            <p>{t("plugins.help.protocolBody")}</p>
                            <pre className="pm-sheet-code">{PROTOCOL}</pre>
                        </section>

                        <section className="pm-sheet-section">
                            <h3>{t("plugins.help.service")}</h3>
                            <p>{t("plugins.help.serviceBody")}</p>
                            <pre className="pm-sheet-code">{SERVICE}</pre>
                        </section>

                        <section className="pm-sheet-section">
                            <h3>{t("plugins.help.settings")}</h3>
                            <p>{t("plugins.help.settingsBody")}</p>
                            <pre className="pm-sheet-code">{SCHEMA}</pre>
                        </section>

                        <p className="pm-sheet-foot">{t("plugins.help.agent")}</p>
                    </div>
                </div>
            </div>
        </Portal>
    );
}
