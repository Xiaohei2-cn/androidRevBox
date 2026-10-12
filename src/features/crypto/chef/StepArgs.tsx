import type { ChefArgDto, ChefOperationDto } from "@/api/chef";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { cn } from "@/lib/utils";
import type { ChefLabels } from "./labels";

interface StepArgsProps {
  operation: ChefOperationDto;
  args: Record<string, string>;
  labels: ChefLabels;
  placeholder: string;
  onChange: (name: string, value: string) => void;
  /** 哪些字段其实被算法忽略了（例如 ECB 不收 IV），标出来比藏起来有用 */
  inertFields?: string[];
  inertLabel: string;
}

/**
 * 值会很长（密钥、正则、私钥）的参数占整行；其余两个一行。
 * 为什么按参数名而不是按控件类型：短的候选值（cbc/pkcs7）并排好看，
 * 而一条 32 字节的 hex key 挤在半行里等于没法读。
 */
const WIDE_ARGS = new Set([
  "aad",
  "alphabet",
  "characters",
  "hash",
  "info",
  "iv",
  "key",
  "pattern",
  "privateKey",
  "publicKey",
  "replacement",
  "salt",
  "search",
  "secret",
]);

/**
 * 一步算法的参数表单。控件由后端注册表决定（text/bool/number/choice），
 * 所以新增算法不需要动这里——只要注册表里写了参数，界面就有。
 *
 * 控件**不写 id**：同一条配方里可以有两个相同算法的步骤，id 会撞车
 * （撞车的 id 让 label 只指向第一个控件，看起来就是"我改的是上面那一步"）。
 * 关联靠嵌套 label + aria-label 两条，都保留。
 */
export function StepArgs({
  operation,
  args,
  labels,
  placeholder,
  onChange,
  inertFields = [],
  inertLabel,
}: StepArgsProps) {
  if (operation.args.length === 0) {
    return <p className="text-xs text-muted-foreground">{placeholder}</p>;
  }
  return (
    <div className="grid grid-cols-2 gap-x-2 gap-y-1.5">
      {operation.args.map((arg) => (
        <Field
          key={arg.name}
          arg={arg}
          value={args[arg.name] ?? arg.default}
          labels={labels}
          inertLabel={inertLabel}
          inert={inertFields.includes(arg.name)}
          wide={WIDE_ARGS.has(arg.name)}
          onChange={(value) => onChange(arg.name, value)}
        />
      ))}
    </div>
  );
}

function Field({
  arg,
  value,
  labels,
  inert,
  inertLabel,
  wide,
  onChange,
}: {
  arg: ChefArgDto;
  value: string;
  labels: ChefLabels;
  inert: boolean;
  inertLabel: string;
  wide: boolean;
  onChange: (value: string) => void;
}) {
  const label = labels.argument(arg.name);
  const hint = inert ? inertLabel : undefined;

  if (arg.kind === "bool") {
    return (
      <label
        title={hint}
        className={cn(
          "flex h-8 items-center gap-1.5 self-end rounded-lg border border-transparent px-1.5 text-xs",
          "hover:bg-accent/70",
          inert && "text-muted-foreground/70 line-through",
          wide && "col-span-2",
        )}
      >
        <input
          type="checkbox"
          checked={value === "true"}
          onChange={(event) => onChange(event.target.checked ? "true" : "false")}
          className="h-3.5 w-3.5 shrink-0 accent-primary"
        />
        <span className="min-w-0 truncate">{label}</span>
      </label>
    );
  }

  return (
    <label className={cn("flex min-w-0 flex-col gap-1 text-xs", wide && "col-span-2")}>
      <span className="flex min-w-0 items-center gap-1 truncate text-[11px] text-muted-foreground">
        {label}
        {hint && <em className="not-italic text-[10px] opacity-80">({hint})</em>}
      </span>
      {arg.kind === "choice" ? (
        <Select
          aria-label={label}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          containerClassName="min-w-0 max-w-full"
          className="h-7 text-xs"
        >
          {arg.options.map((option) => (
            <option key={option} value={option}>
              {labels.option(option)}
            </option>
          ))}
        </Select>
      ) : (
        <Input
          aria-label={label}
          type={arg.kind === "number" ? "number" : "text"}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          className="h-8 rounded-lg font-mono text-xs"
        />
      )}
    </label>
  );
}
