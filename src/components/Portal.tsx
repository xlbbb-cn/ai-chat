import { useEffect, useState } from "react";
import { createPortal } from "react-dom";

interface Props {
    children: React.ReactNode;
}

/**
 * Renders children into `document.body` via a React portal.
 *
 * Use this for modal/overlay layers that need to escape ancestor stacking
 * contexts. Elements with `transform`, `filter`, `backdrop-filter`,
 * `perspective`, `contain: paint`, or `will-change` create a new containing
 * block for `position: fixed` descendants, so an overlay rendered inside
 * `.sidebar` (which uses `backdrop-filter`) would otherwise be clipped to
 * the sidebar's box.
 */
export function Portal({ children }: Props) {
    const [container, setContainer] = useState<HTMLElement | null>(null);

    useEffect(() => {
        setContainer(document.body);
    }, []);

    if (!container) return null;
    return createPortal(children, container);
}
