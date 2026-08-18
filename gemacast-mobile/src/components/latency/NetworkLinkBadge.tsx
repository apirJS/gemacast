import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';
import type { NetworkLink } from '../../core/types';
import { Usb, Wifi, Globe, Cable, HelpCircle } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

type LinkMeta = {
  icon: LucideIcon;
  label: string;
  color: string;
};

function getLinkMeta(link: NetworkLink): LinkMeta {
  switch (link) {
    case 'adb':
      return { icon: Cable, label: 'ADB', color: 'text-accent-green' };
    case 'usbTether':
      return { icon: Usb, label: 'USB', color: 'text-accent-green' };
    case 'wifi5Ghz':
      return { icon: Wifi, label: '5 GHz', color: 'text-accent-aqua' };
    case 'wifi2_4Ghz':
      return { icon: Wifi, label: '2.4 GHz', color: 'text-accent-yellow' };
    case 'ethernet':
      return { icon: Globe, label: 'Ethernet', color: 'text-accent-aqua' };
    case 'wifiUnknown':
      return { icon: Wifi, label: 'WiFi', color: 'text-muted-foreground' };
    default:
      return { icon: HelpCircle, label: 'Unknown', color: 'text-muted-foreground/60' };
  }
}

type NetworkLinkBadgeProps = {
  /**
   * Render a leading `|` rule. Opt-in because the badge is self-hiding — the
   * caller cannot know whether it will render, so it has to own the separator or
   * a stray divider is left behind when there is no link to show.
   */
  withLeadingSeparator?: boolean;
};

/**
 * The link the buffer is actually tuned for.
 *
 * `LinkPair::effective_link()` on the Rust side picks the *weaker* of the two
 * sides, and that is the side the jitter profile keys on — so when the two
 * differ, the binding side is the only one worth emphasising. When they match,
 * rendering both is pure repetition and we collapse to a single label.
 */
export function NetworkLinkBadge({ withLeadingSeparator = false }: NetworkLinkBadgeProps = {}) {
  const linkPair = useAppStore((s) => s.networkLinkPair);
  const status = useAppStore((s) => s.status);

  const visible =
    linkPair &&
    (status === Status.Connected || status === Status.Playing || status === Status.Paused);

  if (!visible) return null;

  const effective = getLinkMeta(linkPair.effective);
  const EffectiveIcon = effective.icon;

  // Both ends agree, so one label describes the whole path.
  const isSymmetric = linkPair.phone === linkPair.pc;
  const otherSide = linkPair.effective === linkPair.phone ? linkPair.pc : linkPair.phone;
  const other = getLinkMeta(otherSide);

  return (
    <div
      id="network-link-badge"
      className="inline-flex min-w-0 items-center gap-1.5 text-[11px] font-medium"
      title={
        isSymmetric
          ? `Link: ${effective.label}`
          : `Buffer tuned for the weaker side: ${effective.label} (other side ${other.label})`
      }
    >
      {withLeadingSeparator && (
        <span aria-hidden="true" className="text-muted-foreground/40">
          |
        </span>
      )}
      <EffectiveIcon size={12} className={`shrink-0 ${effective.color}`} aria-hidden="true" />
      <span className={`truncate ${effective.color}`}>{effective.label}</span>
      {!isSymmetric && (
        <span className="shrink-0 text-[10px] text-muted-foreground/60">via {other.label}</span>
      )}
    </div>
  );
}
