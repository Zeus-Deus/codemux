import { render, screen, fireEvent } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { HermesPermissionOptions } from "./HermesPermissionOptions";
it("preserves actual session and permanent scopes and prevents duplicate submission", () => {
  const onDecide=vi.fn();
  render(<HermesPermissionOptions payload={{options:[{optionId:"allow_session",kind:"allow_always",name:"Allow for session"},{optionId:"allow_always",kind:"allow_always",name:"Allow always"},{optionId:"deny",kind:"reject_once",name:"Deny"}]}} onDecide={onDecide}/>);
  fireEvent.click(screen.getByRole("button",{name:"Allow for session"}));
  fireEvent.click(screen.getByRole("button",{name:"Allow always"}));
  expect(onDecide).toHaveBeenCalledExactlyOnceWith({decision:"provider_option",option_id:"allow_session"});
});
