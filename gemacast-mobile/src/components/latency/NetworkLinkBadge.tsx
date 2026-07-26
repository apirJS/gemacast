import { useAppStore } from '../../stores/app-store';
import { Status } from '../../core/types';
import type { NetworkLink } from '../../core/types';
import { Usb, Wifi, Globe, Cable, HelpCircle, Smartphone, Monitor } from 'lucide-react';
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

export function NetworkLinkBadge() {
  const linkPair = useAppStore((s) => s.networkLinkPair);
  const status = useAppStore((s) => s.status);

  const visible =
    linkPair &&
    (status === Status.Connected || status === Status.Playing || status === Status.Paused);

  if (!visible) return null;

  const phone = getLinkMeta(linkPair.phone);
  const pc = getLinkMeta(linkPair.pc);
  const PhoneIcon = phone.icon;
  const PcIcon = pc.icon;

  return (
    <div
      id="network-link-badge"
      className="flex items-center gap-3 text-[10px] uppercase tracking-wider animate-[fade-in_300ms_ease-out]"
    >
      <span className={`inline-flex items-center gap-1 ${phone.color}`}>
        <Smartphone size={10} className="text-muted-foreground/60 shrink-0" />
        <PhoneIcon size={11} className="shrink-0" />
        <span className="font-medium">{phone.label}</span>
      </span>

      <span className="text-muted-foreground/30 text-[8px]">⟷</span>

      {/* PC side */}
      <span className={`inline-flex items-center gap-1 ${pc.color}`}>
        <Monitor size={10} className="text-muted-foreground/60 shrink-0" />
        <PcIcon size={11} className="shrink-0" />
        <span className="font-medium">{pc.label}</span>
      </span>
    </div>
  );
}
