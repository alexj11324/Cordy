import type { SVGProps } from "react";

export function WeixinMark(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...props}>
      <path
        fill="#07C160"
        d="M9.4 3C4.8 3 1 6.1 1 9.9c0 2.2 1.3 4.2 3.4 5.5l-.8 2.5 2.9-1.5c.9.3 1.9.4 2.9.4h.5a6.1 6.1 0 0 1-.3-1.8c0-3.7 3.6-6.7 8-6.7h.5C17.2 5.3 13.7 3 9.4 3Z"
      />
      <path
        fill="#07C160"
        d="M23 15c0-3.2-3.2-5.8-7.1-5.8S8.8 11.8 8.8 15s3.2 5.8 7.1 5.8c.9 0 1.7-.1 2.5-.4l2.5 1.3-.7-2.2C21.9 18.4 23 16.8 23 15Z"
      />
      <circle cx="6.7" cy="9" r="1" fill="white" />
      <circle cx="12.1" cy="9" r="1" fill="white" />
      <circle cx="13.5" cy="14.3" r=".8" fill="white" />
      <circle cx="18.2" cy="14.3" r=".8" fill="white" />
    </svg>
  );
}
