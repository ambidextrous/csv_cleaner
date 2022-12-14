use chrono::NaiveDate;
use csv::StringRecord;
use rustc_hash::FxHashMap as HashMap;
use serde::Deserialize;
use serde::Serialize;
use std::error::Error;
use std::fs;
use std::fs::File;
use std::io;
use std::io::ErrorKind;
use std::iter::Iterator;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
enum ColumnType {
    String,
    Int,
    Date,
    Float,
    Enum,
    Bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Column {
    column_type: ColumnType,
    illegal_val_replacement: String,
    legal_vals: Vec<String>,
    format: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Schema {
    columns: Vec<Column>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct JsonColumn {
    name: String,
    column_type: ColumnType,
    illegal_val_replacement: Option<String>,
    legal_vals: Option<Vec<String>>,
    format: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JsonSchema {
    columns: Vec<JsonColumn>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ColumnLog {
    name: String,
    invalid_count: i32,
    max_invalid: Option<String>,
    min_invalid: Option<String>,
}

type Record = HashMap<String, String>;

pub fn process_rows(
    input_path: &String,
    output_path: &String,
    log_path: &String,
    schema_path: &String,
    sep: u8,
) -> Result<(), Box<dyn Error>> {
    // Process CSV row by row in memory buffer, writing the output to disk
    // as you go.
    let mut row_count = 0;
    let schema_string = fs::read_to_string(schema_path)?;
    let json_schema: JsonSchema = serde_json::from_str(&schema_string)?;
    let schema_map = generate_validated_schema(json_schema)?;
    let null_vals = get_null_vals();
    let header_input_file = File::open(input_path)?;
    let mut header_rdr = csv::Reader::from_reader(header_input_file);
    let mut wtr = csv::WriterBuilder::new()
        .delimiter(sep)
        .from_path(output_path)?;
    let column_names = header_rdr.headers()?;
    wtr.write_record(&column_names.clone())?;
    let column_string_names: Vec<String> = column_names.iter().map(|x| x.to_string()).collect();
    let column_logs: Vec<ColumnLog> = column_names
        .clone()
        .into_iter()
        .map(|x| ColumnLog {
            name: x.to_string(),
            invalid_count: 0,
            max_invalid: None,
            min_invalid: None,
        })
        .collect();
    let column_log_tuples: Vec<(String, ColumnLog)> = column_string_names
        .clone()
        .into_iter()
        .zip(column_logs.iter().cloned())
        .collect();
    let mut mut_log_map: HashMap<String, ColumnLog> = column_log_tuples.into_iter().collect();
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(sep)
        .from_path(input_path)?;
    for row in rdr.deserialize() {
        row_count += 1;
        let row_map: Record = row?;
        let cleaned_row = process_row(
            column_names,
            &schema_map,
            row_map,
            &mut mut_log_map,
            &null_vals,
        )?;
        wtr.write_record(&cleaned_row)?;
    }
    let log_map_all = jsonify_log_map(mut_log_map.clone(), &row_count);
    let log_error_message = format!("Unable to write JSON log file to `{log_path}`");
    fs::write(log_path, log_map_all).expect(&log_error_message);
    let log_map_errors = jsonify_log_map_errors(mut_log_map, &row_count);
    println!("Finished processing CSV file. Error report:\n{log_map_errors}");

    Ok(())
}

fn process_row<'a>(
    ordered_column_names: &'a StringRecord,
    schema_dict: &'a HashMap<String, Column>,
    row_map: HashMap<String, String>,
    log_map: &'a mut HashMap<String, ColumnLog>,
    null_vals: &Vec<String>,
) -> Result<StringRecord, Box<dyn Error>> {
    // Process a single CSV row
    let mut processed_row = Vec::new();
    for column_name in ordered_column_names {
        let column_value = row_map.get(column_name).ok_or_else(|| {
            format!("Key error, could not find column_name `{column_name}` in row map")
        })?;
        let cleaned_value = column_value.clean(&null_vals);
        let column = schema_dict.get(column_name).ok_or_else(|| {
            format!("Key error, could not find column_name `{column_name}` in schema`")
        })?;
        let processed_value = cleaned_value.process(&column);
        if processed_value != cleaned_value {
            let column_log = log_map.get(column_name).ok_or_else(|| {
                format!("Key error, could not find column_name `{column_name}` in log_map`")
            })?;
            let invalid_count = column_log.invalid_count + 1;
            let mut max_invalid = None::<String>;
            max_invalid = column_log.max_invalid.clone();
            let mut min_invalid = None::<String>;
            min_invalid = column_log.min_invalid.clone();
            match &column_log.max_invalid {
                Some(x) => {
                    if &processed_value > x {
                        max_invalid = Some(cleaned_value.clone());
                    }
                }
                None => {
                    max_invalid = Some(cleaned_value.clone());
                }
            }
            match &column_log.min_invalid {
                Some(x) => {
                    if &processed_value < x {
                        min_invalid = Some(cleaned_value.clone());
                    }
                }
                None => {
                    min_invalid = Some(cleaned_value.clone());
                }
            }
            let column_log_mut = log_map.get_mut(&column_name.to_string()).ok_or_else(|| {
                format!("Key error, could not find column_name `{column_name}` in log_map`")
            })?;
            column_log_mut.invalid_count = invalid_count;
            column_log_mut.min_invalid = min_invalid;
            column_log_mut.max_invalid = max_invalid;
        }
        processed_row.push(processed_value);
    }
    let processed_record = StringRecord::from(processed_row);

    Ok(processed_record)
}

fn jsonify_log_map_errors(log_map: HashMap<String, ColumnLog>, total_rows: &i32) -> String {
    jsonify_log_map_all_or_errors(log_map, total_rows, &true)
}

fn jsonify_log_map(log_map: HashMap<String, ColumnLog>, total_rows: &i32) -> String {
    jsonify_log_map_all_or_errors(log_map, total_rows, &false)
}

fn jsonify_log_map_all_or_errors(
    log_map: HashMap<String, ColumnLog>,
    total_rows: &i32,
    errors_only: &bool,
) -> String {
    let mut combined_string =
        format!("{{\n\t\"total_rows\": {total_rows},\n\t\"columns_with_errors\": [\n\t\t");
    let mut is_first_row = true;
    for (column_name, column_log) in log_map.iter() {
        if column_log.invalid_count > 0 || !errors_only {
            let mut max_val = String::new();
            {
                if let Some(x) = &column_log.max_invalid {
                    max_val = x.clone();
                }
            }
            let mut min_val = String::new();
            {
                if let Some(x) = &column_log.min_invalid {
                    min_val = x.clone();
                }
            }
            let invalid_row_count = column_log.invalid_count.clone();
            let col_string = format!("{{\n\t\t\t\"column_name\": \"{column_name}\",\n\t\t\t\"invalid_row_count\": {invalid_row_count},\n\t\t\t\"max_illegal_val\": \"{max_val}\",\n\t\t\t\"min_illegal_val\": \"{min_val}\"\n\t\t}}");
            if is_first_row {
                combined_string = format!("{combined_string}{col_string}");
            } else {
                combined_string = format!("{combined_string},{col_string}");
            }
            is_first_row = false;
        }
    }
    combined_string = format!("{combined_string}\n\t]\n}}");
    combined_string
}

pub fn generate_validated_schema(
    json_schema: JsonSchema,
) -> Result<HashMap<String, Column>, io::Error> {
    let empty_vec: Vec<String> = Vec::new();
    let empty_string = String::new();
    let mut column_map: HashMap<String, Column> = HashMap::default();
    for column in json_schema.columns {
        let new_col = Column {
            column_type: column.column_type.clone(),
            illegal_val_replacement: column
                .illegal_val_replacement
                .unwrap_or(empty_string.clone()),
            legal_vals: column.legal_vals.unwrap_or(empty_vec.clone()),
            format: column.format.unwrap_or(empty_string.clone()),
        };

        match column.column_type {
            ColumnType::Date => {
                if new_col.format.len() == 0 {
                    let custom_error = io::Error::new(
                        ErrorKind::Other,
                        "Missing required `format` string value for Date column",
                    );
                    return Err(custom_error);
                }
            }
            ColumnType::Enum => {
                if new_col.legal_vals.len() == 0 {
                    let custom_error = io::Error::new(
                        ErrorKind::Other,
                        "Missing required `legal_vals` string list value for Enum column",
                    );
                    return Err(custom_error);
                }
            }
            _ => {}
        }
        column_map.insert(column.name, new_col);
    }
    Ok(column_map)
}

trait Process {
    fn process(&self, column: &Column) -> Self;
}

impl Process for String {
    fn process(&self, column: &Column) -> Self {
        match column.column_type {
            ColumnType::String => self.to_string(),
            ColumnType::Int => {
                let cleaned = self.de_pseudofloat();
                if cleaned.casts_to_int() {
                    cleaned.to_string()
                } else {
                    column.illegal_val_replacement.to_owned()
                }
            }
            ColumnType::Date => {
                let cleaned = self;
                if cleaned.casts_to_date(&column.format) {
                    cleaned.to_string()
                } else {
                    column.illegal_val_replacement.to_owned()
                }
            }
            ColumnType::Float => {
                let cleaned = self;
                if cleaned.casts_to_float() {
                    cleaned.to_string()
                } else {
                    column.illegal_val_replacement.to_owned()
                }
            }
            ColumnType::Enum => {
                let cleaned = self;
                if cleaned.casts_to_enum(&column.legal_vals) {
                    cleaned.to_string()
                } else {
                    column.illegal_val_replacement.to_owned()
                }
            }
            ColumnType::Bool => {
                let cleaned = self;
                if cleaned.casts_to_bool() {
                    cleaned.to_string()
                } else {
                    column.illegal_val_replacement.to_owned()
                }
            }
        }
    }
}

fn get_null_vals() -> Vec<String> {
    // Values that will be treated as NULL, based on the ones used by
    // Pandas: https://pandas.pydata.org/docs/reference/api/pandas.read_csv.html
    let null_vals = vec![
        "#N/A".to_string(),
        "#N/A".to_string(),
        "N/A".to_string(),
        "#NA".to_string(),
        "-1.#IND".to_string(),
        "-1.#QNAN".to_string(),
        "-NaN".to_string(),
        "-nan".to_string(),
        "1.#IND".to_string(),
        "1.#QNAN".to_string(),
        "<NA>".to_string(),
        "N/A".to_string(),
        "NA".to_string(),
        "NULL".to_string(),
        "NaN".to_string(),
        "n/a".to_string(),
        "nan".to_string(),
        "null".to_string(),
    ];
    null_vals
}

trait Clean {
    fn clean(&self, null_vals: &[String]) -> Self;
}

impl Clean for String {
    fn clean(&self, null_vals: &[String]) -> Self {
        if null_vals.contains(self) {
            String::new()
        } else {
            self.to_string()
        }
    }
}

trait CastsToBool {
    fn casts_to_bool(&self) -> bool;
}

impl CastsToBool for String {
    fn casts_to_bool(&self) -> bool {
        let bool_strings = vec![
            "true".to_string(),
            "1".to_string(),
            "1.0".to_string(),
            "yes".to_string(),
            "false".to_string(),
            "0.0".to_string(),
            "0".to_string(),
            "no".to_string(),
        ];
        bool_strings.contains(&self.to_lowercase())
    }
}

trait CastsToEnum {
    fn casts_to_enum(&self, legal_values: &[String]) -> bool;
}

impl CastsToEnum for String {
    fn casts_to_enum(&self, legal_values: &[String]) -> bool {
        legal_values.contains(self)
    }
}

trait CastsToDate {
    fn casts_to_date(&self, format: &String) -> bool;
}

impl CastsToDate for String {
    fn casts_to_date(&self, format: &String) -> bool {
        NaiveDate::parse_from_str(self, format).is_ok()
    }
}

trait CastsToInt {
    fn casts_to_int(&self) -> bool;
}

impl CastsToInt for String {
    fn casts_to_int(&self) -> bool {
        // Note: Will return false is number is too large or small
        // to be stored as a 64 bit signed int:
        //     min val: -9_223_372_036_854_775_808
        //     max val: 9_223_372_036_854_775_807
        self.parse::<i64>().is_ok()
    }
}

trait CastsToFloat {
    fn casts_to_float(&self) -> bool;
}

impl CastsToFloat for String {
    fn casts_to_float(&self) -> bool {
        // Note: Will return false is number is too large or small
        // to be stored as a 64 bit signed float:
        //     min val: -1.7976931348623157E+308f64
        //     max val: 1.7976931348623157E+308f64
        self.parse::<f64>().is_ok()
    }
}

trait DePseudofloat {
    fn de_pseudofloat(&self) -> Self;
}

impl DePseudofloat for String {
    fn de_pseudofloat(&self) -> Self {
        let is_pseudofloat = self.ends_with(".0");
        if is_pseudofloat {
            rem_last_n_chars(self, 2).to_string()
        } else {
            self.to_owned()
        }
    }
}

fn rem_last_n_chars(value: &str, n: i32) -> &str {
    let mut chars = value.chars();
    for _ in 0..n {
        chars.next_back();
    }
    chars.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_csv() {
        let contents = "\
INT_COLUMN,STRING_COLUMN,DATE_COLUMN,ENUM_COLUMN
1,some_string,2020-12-01,V3
-2,<NA>,2020-12-02,V4";
    }

    #[test]
    fn clean_string() {
        // Arrange
        let input = vec!["NULL".to_string(), String::new(), " dog\t".to_string()];
        let expected = vec![String::new(), String::new(), " dog\t".to_string()];
        let null_vals = get_null_vals();
        // Act
        let result = input
            .iter()
            .map(|x| x.clean(&null_vals))
            .collect::<Vec<_>>();
        // Assert
        assert_eq!(result, expected);
    }

    #[test]
    fn converts_to_int() {
        // Arrange
        let input = vec![
            "1".to_string(),
            "-3".to_string(),
            "264634633426".to_string(),
            "dog".to_string(),
            "0.4".to_string(),
            "1.0".to_string(),
        ];
        let expected = vec![true, true, true, false, false, false];
        // Act
        let result = input.iter().map(|x| x.casts_to_int()).collect::<Vec<_>>();
        // Assert
        assert_eq!(result, expected);
    }

    #[test]
    fn de_psuedofloats() {
        // Arrange
        let input = vec![
            String::new(),
            "-3.0".to_string(),
            "264634633426.0".to_string(),
            "dog".to_string(),
            "0.4".to_string(),
            "1.0".to_string(),
        ];
        let expected = vec![
            String::new(),
            "-3".to_string(),
            "264634633426".to_string(),
            "dog".to_string(),
            "0.4".to_string(),
            "1".to_string(),
        ];
        // Act
        let result = input.iter().map(|x| x.de_pseudofloat()).collect::<Vec<_>>();
        // Assert
        assert_eq!(result, expected);
    }

    #[test]
    fn process_string() {
        // Arrange
        let input = vec![String::new(), " foo\t".to_string(), "bar".to_string()];
        let expected = vec![String::new(), " foo\t".to_string(), "bar".to_string()];
        let legal_vals: Vec<String> = Vec::new();
        let column = Column {
            column_type: ColumnType::String,
            illegal_val_replacement: String::new(),
            legal_vals: legal_vals,
            format: String::new(),
        };
        // Act
        let result = input.iter().map(|x| x.process(&column)).collect::<Vec<_>>();
        // Assert
        assert_eq!(result, expected);
    }

    #[test]
    fn process_int() {
        // Arrange
        let input = vec!["1".to_string(), "-2.0".to_string(), "3134.4".to_string()];
        let expected = vec!["1".to_string(), "-2".to_string(), String::new()];
        let legal_vals: Vec<String> = Vec::new();
        let column = Column {
            column_type: ColumnType::Int,
            illegal_val_replacement: String::new(),
            legal_vals: legal_vals,
            format: String::new(),
        };
        // Act
        let result = input.iter().map(|x| x.process(&column)).collect::<Vec<_>>();
        // Assert
        assert_eq!(result, expected);
    }

    #[test]
    fn converts_to_date() {
        // Arrange
        let input = vec![
            "2022-01-31".to_string(),
            "1878-02-03".to_string(),
            "2115-04-42".to_string(),
            "dog".to_string(),
            "31-01-2022".to_string(),
        ];
        let expected = vec![true, true, false, false, false];
        // Act
        let result = input
            .iter()
            .map(|x| x.casts_to_date(&"%Y-%m-%d".to_string()))
            .collect::<Vec<_>>();
        // Assert
        assert_eq!(result, expected);
    }

    #[test]
    fn process_date() {
        // Arrange
        let input = vec![
            "2020-01-01".to_string(),
            " 2200-12-31\t".to_string(),
            String::new(),
        ];
        let expected = vec!["2020-01-01".to_string(), String::new(), String::new()];
        let legal_vals: Vec<String> = Vec::new();
        let column = Column {
            column_type: ColumnType::Date,
            illegal_val_replacement: String::new(),
            legal_vals: legal_vals,
            format: "%Y-%m-%d".to_string(),
        };
        // Act
        let result = input.iter().map(|x| x.process(&column)).collect::<Vec<_>>();
        // Assert
        assert_eq!(result, expected);
    }

    #[test]
    fn converts_to_float() {
        // Arrange
        let input = vec![
            "1.0".to_string(),
            "-3".to_string(),
            "264634633426".to_string(),
            "dog".to_string(),
            "0.4".to_string(),
            String::new(),
        ];
        let expected = vec![true, true, true, false, true, false];
        // Act
        let result = input.iter().map(|x| x.casts_to_float()).collect::<Vec<_>>();
        // Assert
        assert_eq!(result, expected);
    }

    #[test]
    fn process_float() {
        // Arrange
        let input = vec![String::new(), " 0.1\t".to_string(), "123.456".to_string()];
        let expected = vec![String::new(), String::new(), "123.456".to_string()];
        let legal_vals: Vec<String> = Vec::new();
        let column = Column {
            column_type: ColumnType::Float,
            illegal_val_replacement: String::new(),
            legal_vals: legal_vals,
            format: String::new(),
        };
        // Act
        let result = input.iter().map(|x| x.process(&column)).collect::<Vec<_>>();
        // Assert
        assert_eq!(result, expected);
    }

    #[test]
    fn converts_to_enum() {
        // Arrange
        let input = vec![
            "A".to_string(),
            "B".to_string(),
            "C".to_string(),
            "7".to_string(),
            "0.4".to_string(),
            String::new(),
        ];
        let legal = vec!["A".to_string(), "B".to_string()];
        let expected = vec![true, true, false, false, false, false];
        // Act
        let result = input
            .iter()
            .map(|x| x.casts_to_enum(&legal))
            .collect::<Vec<_>>();
        // Assert
        assert_eq!(result, expected);
    }

    #[test]
    fn converts_to_tool() {
        // Arrange
        let input = vec![
            "true".to_string(),
            "false".to_string(),
            "True".to_string(),
            "False".to_string(),
            "0".to_string(),
            "1".to_string(),
            "dog".to_string(),
        ];
        let legal = vec!["A".to_string(), "B".to_string()];
        let expected = vec![true, true, true, true, true, true, false];
        // Act
        let result = input.iter().map(|x| x.casts_to_bool()).collect::<Vec<_>>();
        // Assert
        assert_eq!(result, expected);
    }

    #[test]
    fn process_enum() {
        // Arrange
        let input = vec![String::new(), " A\t".to_string(), "B".to_string()];
        let expected = vec![String::new(), String::new(), "B".to_string()];
        let legal_vals = vec!["A".to_string(), "B".to_string()];
        let column = Column {
            column_type: ColumnType::Enum,
            illegal_val_replacement: String::new(),
            legal_vals: legal_vals,
            format: String::new(),
        };
        // Act
        let result = input.iter().map(|x| x.process(&column)).collect::<Vec<_>>();
        // Assert
        assert_eq!(result, expected);
    }

    #[test]
    fn generate_column() {
        // Arrange
        let raw_schema = r#"
            {
                "columns": [
		    {
			"name": "INT_COLUMN",
			"column_type": "Int",
			"illegal_val_replacement": null,
			"legal_vals": null
		    },
		    {
			"name": "DATE_COLUMN",
			"column_type": "Date",
			"format": "%Y-%m-%d"
		    },
		    {
			"name": "FLOAT_COLUMN",
			"column_type": "Float",
			"illegal_val_replacement": ""
		    },
		    {
			"name": "STRING_COLUMN",
			"column_type": "String"
		    },
		    {
			"name": "BOOL_COLUMN",
			"column_type": "Bool"
		    },
		    {
			"name": "ENUM_COLUMN",
			"column_type": "Enum",
			"illegal_val_replacement": "DEFAULT",
			"legal_vals": ["A", "B", "DEFAULT"]
		    }
                ]
            }"#;
        let json_schema: JsonSchema = serde_json::from_str(raw_schema).unwrap();

        generate_validated_schema(json_schema).unwrap();
    }
}
