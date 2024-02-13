use std::{collections::HashMap, convert::TryInto};

use anyhow::{ensure, Context, Error, Result};
use bytes::Buf;
use nom_derive::{nom, NomLE, Parse};

#[derive(Debug, NomLE)]
struct Header {
    magic: [u8; 4],
    _unused: [u8; 132],
    _record_count: u32,
    _field_count: u32,
    record_size: u32,
    _string_table_size: u32,
    _table_hash: u32,
    _layout_hash: u32,
    _min_id: u32,
    _max_id: u32,
    _locale: u32,
    flags: u16,
    _id_index: u16,
    total_field_count: u32,
    _bitpacked_data_offset: u32,
    _lookup_column_count: u32,
    _field_storage_info_size: u32,
    common_data_size: u32,
    pallet_data_size: u32,
    section_count: u32,
}

#[derive(Debug, NomLE)]
struct SectionHeader {
    _tact_key_hash: u64,
    _file_offset: u32,
    record_count: u32,
    string_table_size: u32,
    _offset_records_end: u32,
    id_list_size: u32,
    relationship_data_size: u32,
    offset_map_id_count: u32,
    copy_table_count: u32,
}

#[derive(Debug, NomLE)]
struct FieldStructure {
    _size: i16,
    _position: u16,
}

#[derive(Debug, NomLE)]
struct FieldStorageInfo {
    _field_offset_bits: u16,
    _field_size_bits: u16,
    _additional_data_size: u32,
    _storage_type: u32,
    _compression1: u32,
    _compression2: u32,
    _compression3: u32,
}

#[derive(Debug, NomLE)]
#[nom(ExtraArgs(header: &Header))]
struct Record {
    #[nom(Count = "header.record_size")]
    data: Vec<u8>,
}

#[derive(Debug, NomLE)]
struct CopyTableEntry {
    _id_of_new_row: u32,
    _id_of_copied_row: u32,
}

#[derive(Debug, NomLE)]
struct OffsetMapEntry {
    _offset: u32,
    _size: u16,
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
    _copy_table: Vec<CopyTableEntry>,
    #[nom(Count = "section_header.offset_map_id_count")]
    _offset_map: Vec<OffsetMapEntry>,
    #[nom(Count = "section_header.relationship_data_size")]
    _relationship_data: Vec<u8>,
}

fn parse_sections<'a>(
    header: &Header,
    section_headers: &[SectionHeader],
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
    _section_headers: Vec<SectionHeader>,
    #[nom(Count = "header.total_field_count")]
    _fields: Vec<FieldStructure>,
    #[nom(Count = "header.total_field_count")]
    _field_info: Vec<FieldStorageInfo>,
    #[nom(Count = "header.pallet_data_size")]
    _pallet_data: Vec<u8>,
    #[nom(Count = "header.common_data_size")]
    _common_data: Vec<u8>,
    #[nom(Parse = "|i| parse_sections(&header, &_section_headers, i)")]
    sections: Vec<Section>,
}

pub(crate) fn strings(data: &[u8]) -> Result<HashMap<u32, Vec<String>>> {
    let File {
        mut sections,
        header:
            Header {
                magic,
                flags,
                record_size,
                ..
            },
        ..
    } = File::parse(data).map_err(|_| Error::msg("parse error"))?.1;
    ensure!(magic == *b"WDC5", "unsupported magic");
    ensure!(flags == 4, "unsupported flags");
    ensure!(sections.len() == 1, "unsupported number of sections");
    let Section {
        records,
        id_list,
        string_table,
        ..
    } = sections.remove(0);
    let num_records = records.len();
    ensure!(id_list.len() == num_records, "unexpected record count");
    let rsize: usize = record_size.try_into()?;
    ensure!(rsize % 4 == 0, "unexpected record size");
    let values = records
        .into_iter()
        .enumerate()
        .map(|(k, rec)| {
            (0..rsize)
                .step_by(4)
                .map(|offset| {
                    let value: usize = (&rec.data.as_slice()[offset..]).get_u32_le().try_into()?;
                    String::from_utf8(
                        string_table
                            .iter()
                            .skip(value - (num_records - k) * rsize + offset)
                            .take_while(|&b| *b != 0)
                            .cloned()
                            .collect(),
                    )
                    .context("wdc3 string field parsing")
                })
                .collect::<Result<Vec<_>>>()
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(id_list.into_iter().zip(values).collect())
}
