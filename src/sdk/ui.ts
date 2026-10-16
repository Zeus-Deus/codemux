/**
 * The app's own UI primitives, for plugins. Style with these and with theme
 * CSS variables (`host.theme.cssVar(role)`); Tailwind only generates classes
 * it finds in Codemux's own source, so arbitrary utility classes in a plugin
 * may not exist.
 */
export { Button, buttonVariants } from "@/components/ui/button";
export { Input } from "@/components/ui/input";
export { Textarea } from "@/components/ui/textarea";
export { Label } from "@/components/ui/label";
export { Switch } from "@/components/ui/switch";
export { Checkbox } from "@/components/ui/checkbox";
export {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
export { Popover, PopoverAnchor, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
export { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
export {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
export { Badge, badgeVariants } from "@/components/ui/badge";
export { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
export { ScrollArea, ScrollBar } from "@/components/ui/scroll-area";
export { Separator } from "@/components/ui/separator";
export { Kbd } from "@/components/ui/kbd";
export { Spinner } from "@/components/ui/spinner";
export { cn } from "@/lib/utils";
