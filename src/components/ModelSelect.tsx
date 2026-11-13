import { useI18n, type MessageKey } from "../i18n";
import type { ModelMetadata } from "../types";
import {
    FontAwesomeIcon,
    faBrain,
    faEye,
    faFileLines,
    faScrewdriverWrench,
    faVolumeHigh,
    type IconDefinition,
} from "../icons";
import { Dropdown, type DropdownOption } from "./Dropdown";
import "./ModelSelect.css";

interface Props {
    /** Model ids offered in the list (config `model_catalog`). */
    models: string[];
    /** Currently selected model id. */
    value: string;
    /** Capability metadata per model id, from the merged catalogue. */
    metadata: Record<string, ModelMetadata>;
    disabled?: boolean;
    onChange: (model: string) => void;
}

/**
 * Capability badges rendered inside every option. The labels reuse the
 * `settings.api.capability*` strings (Settings → API shows the same set) so the
 * two views can never drift apart.
 */
const CAPABILITY_CHIPS: { flag: keyof ModelMetadata; icon: IconDefinition; labelKey: MessageKey }[] = [
    { flag: "supports_tools", icon: faScrewdriverWrench, labelKey: "settings.api.capabilityTools" },
    { flag: "supports_reasoning", icon: faBrain, labelKey: "settings.api.capabilityReasoning" },
    { flag: "supports_vision", icon: faEye, labelKey: "settings.api.capabilityVision" },
    { flag: "supports_files", icon: faFileLines, labelKey: "settings.api.capabilityFiles" },
    { flag: "supports_audio", icon: faVolumeHigh, labelKey: "settings.api.capabilityAudio" },
];

/** Badges for one model, in display order (empty when nothing is known). */
function chipsFor(metadata: ModelMetadata | undefined) {
    return metadata ? CAPABILITY_CHIPS.filter((chip) => metadata[chip.flag]) : [];
}

/**
 * Width of the popup when the trigger is narrower — wide enough for a model id
 * plus four labelled capability badges on a single line.
 */
const MIN_POPUP_WIDTH = 380;
/** Popup height is capped, so long catalogues scroll instead of overflowing. */
const MAX_POPUP_HEIGHT = 320;

/**
 * The model picker: the shared `Dropdown` with capability badges on its rows.
 *
 * A native `<select>` cannot render markup inside `<option>`, and its list is
 * drawn by the platform, so the picker is the generic dropdown primitive with a
 * wider floor and an upward popup — the composer row lives at the bottom of the
 * window, where there is no room below.
 */
export function ModelSelect({ models, value, metadata, disabled, onChange }: Props) {
    const { t } = useI18n();

    const options: DropdownOption[] = models.map((model) => {
        const chips = chipsFor(metadata[model]);
        return {
            value: model,
            label: model,
            // Icon only — the capability name lives in the tooltip and in the
            // accessible name, so five badges stay one text line wide.
            extra:
                chips.length > 0 &&
                chips.map((chip) => (
                    <span
                        key={chip.flag}
                        className="model-capability-chip"
                        role="img"
                        title={t(chip.labelKey)}
                        aria-label={t(chip.labelKey)}
                    >
                        <FontAwesomeIcon icon={chip.icon} />
                    </span>
                )),
        };
    });

    return (
        <Dropdown
            className="model-select-trigger"
            label={t("app.selectModel")}
            compact
            placement="up"
            minPopupWidth={MIN_POPUP_WIDTH}
            maxPopupHeight={MAX_POPUP_HEIGHT}
            options={options}
            value={value}
            disabled={disabled}
            onChange={onChange}
        />
    );
}
