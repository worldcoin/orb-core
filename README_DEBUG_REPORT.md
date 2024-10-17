# Sign-up Data

This document explains how we keep track of what data is being collected by the Orb, during a _sign-up_.
The `struct` that is responsible for this data is [DebugReport][1].

## Files Tracking Sign-up Data Updates

In order to describe which fields have been updated,
we associate each commit with 2 files: `debug_report_schema.json` and `debug_report_schema.csv`.
With every commit pushed into `master` or `prod` branch,
these files are uploaded:

1. to [wld-signup-data-schema][2] AWS S3 Bucket.
2. as GitHub Actions [artifacts](https://docs.github.com/en/actions/using-workflows/storing-workflow-data-as-artifacts).

### Schema Files Description

* `debug_report_schema.json`:
Contains the [JSON schema](https://json-schema.org/) of [DebugReport][1] `struct`.
A `JSON Schema` file can be visualized as a `schema-tree` [here](https://navneethg.github.io/jsonschemaviewer/).

* `debug_report_schema.csv`:
Contains a list of paths from root to each node of the (above) `schema tree`.
This representation is useful when someone wants to annotate additional information for each `data field`.
The columns of this CSV file are `path` and `instance_type`.

### Downloading Schema Files

Steps:

1. Go to [wld-signup-data-schema][2] AWS S3 Bucket.
   * If you do not have access, request access in `#infrastructure` channel on Slack.
2. From the list, select the `.zip` file you are interested in. Each filename contains the date and the corresponding commit hash that produced the schema files.
3. Download the `.zip` file.
4. The `.zip` contains both `debug_report_schema.json` and `debug_report_schema.csv` files.

## Track Changes Between Commits

Let `debug_report_schema_old.csv` and `debug_report_schema_new.csv` be the schema files of two different commits.

The command:

```bash
diff -w --color debug_report_schema_old.csv debug_report_schema_new.csv
```

will print which fields were added/removed between the two commits.

* Lines that start with `>` are **new** fields.
* Lines that start with `<` are **deleted** fields

_Caution_: Make sure that in the above command the first file is the older version.

Example output:

```bash
211a212
> metadata/backend_config/IrisModelConfig,"[String, Null]"
219,221d219
< metadata/backend_config/MaxOffgazeThreshold,"[Number, Null]"
< metadata/backend_config/MaxPupilToIrisRatioThreshold,"[Number, Null]"
< metadata/backend_config/MinOcclusion30Threshold,"[Number, Null]"
223,224d220
< metadata/backend_config/MinOcclusion90Threshold,"[Number, Null]"
< metadata/backend_config/MinPupilToIrisRatioThreshold,"[Number, Null]"
232d227
< metadata/backend_config/UseTokyoPipeline,"[Boolean, Null]"
750,755c745
< metadata/mega_agent_one_config/iris/max_offgaze_threshold,"[Number, Null]"
< metadata/mega_agent_one_config/iris/max_pupil_to_iris_ratio_threshold,"[Number, Null]"
< metadata/mega_agent_one_config/iris/min_occlusion_30_threshold,"[Number, Null]"
< metadata/mega_agent_one_config/iris/min_occlusion_90_threshold,"[Number, Null]"
< metadata/mega_agent_one_config/iris/min_pupil_to_iris_ratio_threshold,"[Number, Null]"
< metadata/mega_agent_one_config/iris/use_tokyo_pipeline,Boolean
---
> metadata/mega_agent_one_config/iris/config,"[String, Null]"
```

### Maintaining Your Own Spreadsheet

If you maintain your own spreadsheet of _sign-up data_ and you want to update
it, then you cannot apply the above command directly.

**Step 1**: With the spreadsheet software of your preference,
export a CSV file containing **only** the `path` and `instance_type` columns.

**Step 2**: After downloading the newest version of `debug_report_schema.csv`,
execute the above command.

**Step 3**: Remove the rows indicated by the command from your spreadsheet.

**Step 4**: Add the rows indicated by the command to your spreadsheet.

**Step 5**: Make sure that your spreadsheet's rows are sorted based on `path`.

**Extra Step**: To be sure that you applied the changes correctly, execute **Step 1** & **Step 2** again. The command should now produce **no** output.

[1]: https://github.com/worldcoin/orb-core/blob/13780451ddde7f2683d2de7d0cac351b948457b5/src/debug_report.rs#L53

[2]: https://s3.console.aws.amazon.com/s3/buckets/wld-signup-data-schema
