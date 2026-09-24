/**
 * Single import surface for the app's Font Awesome icon set.
 *
 * Icons are inline SVG, not a font and not OS emoji: a glyph like ✕ or 📄 is
 * drawn by whatever emoji/text font the platform ships (Apple Color Emoji,
 * Segoe UI Emoji, Noto Color Emoji), so the same button looks different — and
 * sometimes changes size — on macOS, Windows and Linux. Font Awesome renders
 * identical geometry everywhere.
 *
 * Import from here rather than from the `@fortawesome/*` packages directly:
 * the set stays curated, and every icon listed below is referenced by name so
 * it is tree-shaken into the bundle (no full icon library is shipped).
 */
export { FontAwesomeIcon } from "@fortawesome/react-fontawesome";

/** Type of an icon definition, for tables that store icons as data. */
export type { IconDefinition } from "@fortawesome/fontawesome-svg-core";

export {
    faBolt,
    faBoxArchive,
    faBoxOpen,
    faBrain,
    faCaretDown,
    faCheck,
    faChevronDown,
    faChevronRight,
    faChevronUp,
    faCircle,
    faCircleCheck,
    faCircleHalfStroke,
    faCircleXmark,
    faClockRotateLeft,
    faCodeBranch,
    faEye,
    faFileArrowDown,
    faFileLines,
    faFilePen,
    faGear,
    faListCheck,
    faPaperclip,
    faPen,
    faPenToSquare,
    faPlug,
    faRobot,
    faRotateRight,
    faScrewdriverWrench,
    faScroll,
    faSpinner,
    faStar,
    faStopwatch,
    faTrash,
    faTriangleExclamation,
    faVolumeHigh,
    faWandMagicSparkles,
    faWandSparkles,
    faXmark,
} from "@fortawesome/free-solid-svg-icons";

/** Outlined counterpart of `faStar`, used for "not a favorite yet". */
export { faStar as faStarOutline } from "@fortawesome/free-regular-svg-icons";
