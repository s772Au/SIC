# 便携源路径与私有挂载

公开`sources-lock.json`、`test-catalog.json`和验证报告只保存`PROJECT_ROOT`相对路径。数据文件的SHA-256不因本机挂载位置变化而变化；本地文件不必移动或复制。

`source_paths.resolve_source()`的读取优先级为：

1. 显式`SIC_SOURCE_OVERRIDES`指定的私有JSON；按source ID映射到本地文件。
2. `SIC_DATA_ROOT`指定的数据根目录；目录布局为`DATA_ROOT/corpusqa/`、`beam10m/`、`babilong/`、`longmemeval-v2/`。
3. 自动读取`PROJECT_ROOT/.local/source-overrides.json`。
4. `PROJECT_ROOT`加公开清单中的相对路径。

显式配置文件缺少某个ID时，继续使用数据根或公开相对路径；不会再回落到自动私有配置。相对override值以该私有JSON所在目录为基准。显式`SIC_DATA_ROOT`覆盖自动`.local`挂载，不悄悄复用另一台机器的源。

可以将`source-overrides.example.json`复制到`.local/source-overrides.json`，再在私有副本里把`DATA_ROOT`占位符替换为实际本地路径。实际用户名、用户目录或挂载地址仅保存在被忽略的`.local`文件中，不提交、不写进验证报告和控制ZIP。也可以在进程环境中设置：

```powershell
$env:SIC_DATA_ROOT = 'DATA_ROOT'
python benchmarks/verify_sources.py --hash --all-locked
```

`DATA_ROOT`和`PROJECT_ROOT`是文档占位符；环境变量示例必须先替换。解析后的绝对路径只用于本地I/O，公共报告始终使用清单的相对`path`，不输出`resolved_path`。数据根缺失、尺寸或摘要不符时校验失败，不自动下载或接受`.part`文件。
