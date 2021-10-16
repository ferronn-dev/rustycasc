use anyhow::{Context, Error, Result};
use nom_derive::{nom, NomLE, Parse};

#[derive(Debug, NomLE)]
struct Header {
    magic: [u8; 4],
    record_count: u32,
    field_count: u32,
    record_size: u32,
    string_table_size: u32,
    table_hash: u32,
    layout_hash: u32,
    min_id: u32,
    max_id: u32,
    locale: u32,
    flags: u16,
    id_index: u16,
    total_field_count: u32,
    bitpacked_data_offset: u32,
    lookup_column_count: u32,
    field_storage_info_size: u32,
    common_data_size: u32,
    pallet_data_size: u32,
    section_count: u32,
}

#[derive(Debug, NomLE)]
struct SectionHeader {
    tact_key_hash: u64,
    file_offset: u32,
    record_count: u32,
    string_table_size: u32,
    offset_records_end: u32,
    id_list_size: u32,
    relationship_data_size: u32,
    offset_map_id_count: u32,
    copy_table_count: u32,
}

#[derive(Debug, NomLE)]
struct FieldStructure {
    size: i16,
    position: u16,
}

#[derive(Debug, NomLE)]
struct FieldStorageInfo {
    field_offset_bits: u16,
    field_size_bits: u16,
    additional_data_size: u32,
    storage_type: u32,
    compression1: u32,
    compression2: u32,
    compression3: u32,
}

#[derive(Debug, NomLE)]
#[nom(ExtraArgs(header: &Header))]
struct Record {
    #[nom(Count = "header.record_size")]
    data: Vec<u8>,
}

#[derive(Debug, NomLE)]
struct CopyTableEntry {
    id_of_new_row: u32,
    id_of_copied_row: u32,
}

#[derive(Debug, NomLE)]
struct OffsetMapEntry {
    offset: u32,
    size: u16,
}

#[derive(Debug, NomLE)]
#[nom(ExtraArgs(header: &Header, section_header: &SectionHeader))]
struct Section {
    #[nom(
        Count = "section_header.record_count",
        Parse = "|i| Record::parse(i, header)"
    )]
    records: Vec<Record>,
    #[nom(Count = "section_header.string_table_size")]
    string_table: Vec<u8>,
    #[nom(Count = "(section_header.id_list_size / 4) as usize")]
    id_list: Vec<u32>,
    #[nom(Count = "section_header.copy_table_count")]
    copy_table: Vec<CopyTableEntry>,
    #[nom(Count = "section_header.offset_map_id_count")]
    offset_map: Vec<OffsetMapEntry>,
    #[nom(Count = "section_header.relationship_data_size")]
    relationship_data: Vec<u8>,
}

fn parse_sections<'a>(
    header: &Header,
    section_headers: &Vec<SectionHeader>,
    mut i: &'a [u8],
) -> nom::IResult<&'a [u8], Vec<Section>> {
    let mut v = Vec::<Section>::new();
    for h in section_headers {
        let (j, section) = Section::parse(i, header, h)?;
        v.push(section);
        i = j;
    }
    Ok((i, v))
}

#[derive(Debug, NomLE)]
#[nom(Complete)]
struct File {
    header: Header,
    #[nom(Count = "header.section_count")]
    section_headers: Vec<SectionHeader>,
    #[nom(Count = "header.total_field_count")]
    fields: Vec<FieldStructure>,
    #[nom(Count = "header.total_field_count")]
    field_info: Vec<FieldStorageInfo>,
    #[nom(Count = "header.pallet_data_size")]
    pallet_data: Vec<u8>,
    #[nom(Count = "header.common_data_size")]
    common_data: Vec<u8>,
    #[nom(Parse = "|i| parse_sections(&header, &section_headers, i)")]
    sections: Vec<Section>,
}

pub fn strings(data: &[u8]) -> Result<Vec<String>> {
    File::parse(data)
        .map_err(|_| Error::msg("parse error"))?
        .1
        .sections[0]
        .string_table
        .split(|b| *b == 0)
        .map(|s| String::from_utf8(s.to_vec()).context("parsing tocdata"))
        .collect::<Result<Vec<String>>>()
}
