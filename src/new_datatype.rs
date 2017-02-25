use error::Result;
use handle::{Handle, ID, FromID, get_id_type};
use object::Object;
use types::{
    TypeDescriptor, H5Type, IntSize, FloatSize, EnumMember,
    EnumType, CompoundField, CompoundType
};
use util::{to_cstring, string_from_cstr};

use libc::{c_char, c_void};

use ffi::h5::hsize_t;
use ffi::h5i::{hid_t, H5I_DATATYPE};
use ffi::h5t::{
    H5Tcreate, H5Tset_size, H5Tinsert, H5Tenum_create, H5Tenum_insert, H5Tcopy,
    H5Tarray_create2, H5T_str_t, H5Tset_strpad, H5T_cset_t, H5Tset_cset, H5Tvlen_create,
    H5Tget_class, H5T_VARIABLE, H5T_class_t, H5Tget_size, H5Tget_sign, H5Tget_nmembers,
    H5Tget_super, H5Tget_member_name, H5Tget_member_type, H5Tget_member_offset,
    H5Tget_member_value, H5Tget_array_ndims, H5Tget_array_dims2, H5Tis_variable_str,
    H5Tget_cset
};

#[cfg(target_endian = "big")]
use globals::{
    H5T_STD_I8BE, H5T_STD_I16BE,
    H5T_STD_I32BE, H5T_STD_I64BE,
    H5T_STD_U8BE, H5T_STD_U16BE,
    H5T_STD_U32BE, H5T_STD_U64BE,
    H5T_IEEE_F32BE, H5T_IEEE_F64BE,
};

#[cfg(target_endian = "little")]
use globals::{
    H5T_STD_I8LE, H5T_STD_I16LE,
    H5T_STD_I32LE, H5T_STD_I64LE,
    H5T_STD_U8LE, H5T_STD_U16LE,
    H5T_STD_U32LE, H5T_STD_U64LE,
    H5T_IEEE_F32LE, H5T_IEEE_F64LE,
};

use globals::{H5T_NATIVE_INT8, H5T_C_S1};

pub struct Datatype {
    handle: Handle,
}

#[doc(hidden)]
impl ID for Datatype {
    fn id(&self) -> hid_t {
        self.handle.id()
    }
}

#[doc(hidden)]
impl FromID for Datatype {
    fn from_id(id: hid_t) -> Result<Datatype> {
        h5lock_s!(match get_id_type(id) {
            H5I_DATATYPE => Ok(Datatype { handle: try!(Handle::new(id)) }),
            _ => Err(From::from(format!("Invalid datatype id: {}", id))),
        })
    }
}

impl Object for Datatype { }

fn datatype_to_descriptor(datatype: &Datatype) -> Result<TypeDescriptor> {
    use ffi::h5t::H5T_class_t::*;
    use ffi::h5t::H5T_sign_t::*;
    use types::TypeDescriptor::*;

    h5lock!({
        let id = datatype.id();
        let size = h5try_s!(H5Tget_size(id)) as usize;
        match H5Tget_class(id) {
            H5T_INTEGER => {
                let signed = match H5Tget_sign(id) {
                    H5T_SGN_NONE => false,
                    H5T_SGN_2 => true,
                    _ => return Err("Invalid sign of integer datatype".into())
                };
                let size = try!(IntSize::from_int(size)
                                .ok_or("Invalid size of integer datatype"));
                Ok(if signed {
                    TypeDescriptor::Integer(size)
                } else {
                    TypeDescriptor::Unsigned(size)
                })
            },
            H5T_FLOAT => {
                let size = try!(FloatSize::from_int(size)
                                .ok_or("Invalid size of float datatype"));
                Ok(TypeDescriptor::Float(size))
            },
            H5T_ENUM => {
                let mut members: Vec<EnumMember> = Vec::new();
                for idx in 0 .. h5try_s!(H5Tget_nmembers(id)) as u32 {
                    let mut value: u64 = 0;
                    h5try_s!(H5Tget_member_value(
                        id, idx, &mut value as *mut _ as *mut c_void
                    ));
                    let name = H5Tget_member_name(id, idx);
                    members.push(EnumMember { name: string_from_cstr(name), value: value });
                    ::libc::free(name as *mut c_void);
                }
                let base_dt = try!(Datatype::from_id(H5Tget_super(id)));
                let (size, signed) = try!(match try!(base_dt.as_type_descriptor()) {
                    Integer(size) => Ok((size, true)),
                    Unsigned(size) => Ok((size, false)),
                    _ => Err("Invalid base type for enum datatype"),
                });
                let bool_members = [
                    EnumMember { name: "FALSE".to_owned(), value: 0 },
                    EnumMember { name: "TRUE".to_owned(), value: 1 },
                ];
                if size == IntSize::U1 && members == bool_members {
                    Ok(TypeDescriptor::Boolean)
                } else {
                    Ok(TypeDescriptor::Enum(
                        EnumType { size: size, signed: signed, members : members }
                    ))
                }
            },
            H5T_COMPOUND => {
                let mut fields: Vec<CompoundField> = Vec::new();
                for idx in 0 .. h5try_s!(H5Tget_nmembers(id)) as u32 {
                    let name = H5Tget_member_name(id, idx);
                    let offset = h5try_s!(H5Tget_member_offset(id, idx)) as usize;
                    let ty = try!(Datatype::from_id(h5try_s!(H5Tget_member_type(id, idx))));
                    fields.push(CompoundField {
                        name: string_from_cstr(name),
                        ty: try!(ty.as_type_descriptor()),
                        offset: offset
                    });
                    ::libc::free(name as *mut c_void);
                }
                Ok(TypeDescriptor::Compound(CompoundType { fields: fields, size: size }))
            },
            H5T_ARRAY => {
                let base_dt = try!(Datatype::from_id(H5Tget_super(id)));
                let ndims = h5try_s!(H5Tget_array_ndims(id));
                if ndims == 1 {
                    let mut len: hsize_t = 0;
                    h5try_s!(H5Tget_array_dims2(id, &mut len as *mut hsize_t));
                    Ok(TypeDescriptor::FixedArray(
                        Box::new(try!(base_dt.as_type_descriptor())), len as usize
                    ))
                } else {
                    Err("Multi-dimensional array datatypes are not supported".into())
                }
            },
            H5T_STRING => {
                let is_variable = h5try_s!(H5Tis_variable_str(id)) == 1;
                let encoding = h5lock_s!(H5Tget_cset(id));
                match (is_variable, encoding) {
                    (false, H5T_cset_t::H5T_CSET_ASCII) => Ok(TypeDescriptor::FixedAscii(size)),
                    (false, H5T_cset_t::H5T_CSET_UTF8) => Ok(TypeDescriptor::FixedUnicode(size)),
                    (true, H5T_cset_t::H5T_CSET_ASCII) => Ok(TypeDescriptor::VarLenAscii),
                    (true, H5T_cset_t::H5T_CSET_UTF8) => Ok(TypeDescriptor::VarLenUnicode),
                    _ => Err("Invalid encoding for string datatype".into())
                }
            },
            H5T_VLEN => {
                let base_dt = try!(Datatype::from_id(H5Tget_super(id)));
                Ok(TypeDescriptor::VarLenArray(Box::new(try!(base_dt.as_type_descriptor()))))
            },
            _ => Err("Unsupported datatype class".into())
        }
    })
}

impl Datatype {
    pub fn as_type_descriptor(&self) -> Result<TypeDescriptor> {
        datatype_to_descriptor(self)
    }
}

pub trait ToDatatype {
    fn to_datatype() -> Result<Datatype>;
}

#[cfg(target_endian = "big")]
macro_rules! be_le {
    ($be:expr, $le:expr) => (h5try_s!(H5Tcopy(*$be)))
}

#[cfg(target_endian = "little")]
macro_rules! be_le {
    ($be:expr, $le:expr) => (h5try_s!(H5Tcopy(*$le)))
}

pub fn datatype_from_descriptor(desc: &TypeDescriptor) -> Result<Datatype> {
    use types::TypeDescriptor::*;

    unsafe fn string_type(size: Option<usize>, encoding: H5T_cset_t) -> Result<hid_t> {
        let string_id = h5try_s!(H5Tcopy(*H5T_C_S1));
        let padding = if size.is_none() {
            H5T_str_t::H5T_STR_NULLPAD
        } else {
            H5T_str_t::H5T_STR_NULLTERM
        };
        let size = size.unwrap_or(H5T_VARIABLE);
        h5try_s!(H5Tset_cset(string_id, encoding));
        h5try_s!(H5Tset_strpad(string_id, padding));
        h5try_s!(H5Tset_size(string_id, size));
        Ok(string_id)
    }

    let datatype_id: Result<_> = h5lock!({
        match *desc {
            Integer(size) => Ok(match size {
                IntSize::U1 => be_le!(H5T_STD_I8BE, H5T_STD_I8LE),
                IntSize::U2 => be_le!(H5T_STD_I16BE, H5T_STD_I16LE),
                IntSize::U4 => be_le!(H5T_STD_I32BE, H5T_STD_I32LE),
                IntSize::U8 => be_le!(H5T_STD_I64BE, H5T_STD_I64LE),
            }),
            Unsigned(size) => Ok(match size {
                IntSize::U1 => be_le!(H5T_STD_U8BE, H5T_STD_U8LE),
                IntSize::U2 => be_le!(H5T_STD_U16BE, H5T_STD_U16LE),
                IntSize::U4 => be_le!(H5T_STD_U32BE, H5T_STD_U32LE),
                IntSize::U8 => be_le!(H5T_STD_U64BE, H5T_STD_U64LE),
            }),
            Float(size) => Ok(match size {
                FloatSize::U4 => be_le!(H5T_IEEE_F32BE, H5T_IEEE_F32LE),
                FloatSize::U8 => be_le!(H5T_IEEE_I16BE, H5T_IEEE_F64LE),
            }),
            Boolean => {
                let bool_id = h5try_s!(H5Tenum_create(*H5T_NATIVE_INT8));
                h5try_s!(H5Tenum_insert(bool_id, b"FALSE\0".as_ptr() as *const c_char,
                                        &0i8 as *const i8 as *const c_void));
                h5try_s!(H5Tenum_insert(bool_id, b"TRUE\0".as_ptr() as *const c_char,
                                        &1i8 as *const i8 as *const c_void));
                Ok(bool_id)
            },
            Enum(ref enum_type) => {
                let base = try!(datatype_from_descriptor(&enum_type.base_type()));
                let enum_id = h5try_s!(H5Tenum_create(base.id()));
                for member in &enum_type.members {
                    let name = try!(to_cstring(member.name.as_ref()));
                    h5try_s!(H5Tenum_insert(enum_id, name.as_ptr(),
                                            &member.value as *const u64 as *const c_void));
                }
                Ok(enum_id)
            },
            Compound(ref compound_type) => {
                let compound_id = h5try_s!(H5Tcreate(H5T_class_t::H5T_COMPOUND, 1));
                for field in &compound_type.fields {
                    let name = try!(to_cstring(field.name.as_ref()));
                    let field_dt = try!(datatype_from_descriptor(&field.ty));
                    h5try_s!(H5Tset_size(compound_id, field.offset + field.ty.size()));
                    h5try_s!(H5Tinsert(compound_id, name.as_ptr(), field.offset, field_dt.id()));
                }
                h5try_s!(H5Tset_size(compound_id, compound_type.size));
                Ok(compound_id)
            },
            FixedArray(ref ty, len) => {
                let elem_dt = try!(datatype_from_descriptor(&ty));
                let dims = len as hsize_t;
                Ok(h5try_s!(H5Tarray_create2(elem_dt.id(), 1, &dims as *const hsize_t)))
            },
            FixedAscii(size) => {
                string_type(Some(size), H5T_cset_t::H5T_CSET_ASCII)
            },
            FixedUnicode(size) => {
                string_type(Some(size), H5T_cset_t::H5T_CSET_UTF8)
            },
            VarLenArray(ref ty) => {
                let elem_dt = try!(datatype_from_descriptor(&ty));
                Ok(h5try_s!(H5Tvlen_create(elem_dt.id())))
            },
            VarLenAscii => {
                string_type(None, H5T_cset_t::H5T_CSET_ASCII)
            },
            VarLenUnicode => {
                string_type(None, H5T_cset_t::H5T_CSET_UTF8)
            },
        }
    });

    Datatype::from_id(try!(datatype_id))
}

impl<T: H5Type> ToDatatype for T {
    fn to_datatype() -> Result<Datatype> {
        datatype_from_descriptor(&T::type_descriptor())
    }
}

#[cfg(test)]
pub mod tests {
    use super::ToDatatype;
    use types::*;


    macro_rules! check_roundtrip {
        ($ty:ty, $desc:expr) => ({
            let desc = <$ty as H5Type>::type_descriptor();
            assert_eq!(desc, $desc);
            let dt = <$ty as ToDatatype>::to_datatype().unwrap();
            assert_eq!(desc, dt.as_type_descriptor().unwrap());
        })
    }

    #[test]
    pub fn test_datatype_roundtrip() {
        check_roundtrip!(i8, TypeDescriptor::Integer(IntSize::U1));
        check_roundtrip!(i16, TypeDescriptor::Integer(IntSize::U2));
        check_roundtrip!(i32, TypeDescriptor::Integer(IntSize::U4));
        check_roundtrip!(i64, TypeDescriptor::Integer(IntSize::U8));
        check_roundtrip!(u8, TypeDescriptor::Unsigned(IntSize::U1));
        check_roundtrip!(u16, TypeDescriptor::Unsigned(IntSize::U2));
        check_roundtrip!(u32, TypeDescriptor::Unsigned(IntSize::U4));
        check_roundtrip!(u64, TypeDescriptor::Unsigned(IntSize::U8));
        check_roundtrip!(f32, TypeDescriptor::Float(FloatSize::U4));
        check_roundtrip!(f64, TypeDescriptor::Float(FloatSize::U8));
        check_roundtrip!(bool, TypeDescriptor::Boolean);
        check_roundtrip!([bool; 5], TypeDescriptor::FixedArray(Box::new(TypeDescriptor::Boolean), 5));
        check_roundtrip!(VarLenArray<bool>, TypeDescriptor::VarLenArray(Box::new(TypeDescriptor::Boolean)));
        check_roundtrip!(FixedAscii<[_; 5]>, TypeDescriptor::FixedAscii(5));
        check_roundtrip!(FixedUnicode<[_; 5]>, TypeDescriptor::FixedUnicode(5));
        check_roundtrip!(VarLenAscii, TypeDescriptor::VarLenAscii);
        check_roundtrip!(VarLenUnicode, TypeDescriptor::VarLenUnicode);
        h5def!(#[repr(i64)] enum X { A = 1, B = -2 });
        let x_desc = TypeDescriptor::Enum(EnumType {
            size: IntSize::U8,
            signed: true,
            members: vec![
                EnumMember { name: "A".into(), value: 1 },
                EnumMember { name: "B".into(), value: -2i64 as u64 },
            ]
        });
        check_roundtrip!(X, x_desc);
        h5def!(struct A { a: i64, b: u64 });
        let a_desc = TypeDescriptor::Compound(CompoundType {
            fields: vec![
                CompoundField { name: "a".into(), ty: i64::type_descriptor(), offset: 0 },
                CompoundField { name: "b".into(), ty: u64::type_descriptor(), offset: 8 },
            ],
            size: 16,
        });
        check_roundtrip!(A, a_desc);
        h5def!(struct C {
            a: [X; 2],
            b: [[A; 4]; 32],
        });
        let c_desc = TypeDescriptor::Compound(CompoundType {
            fields: vec![
                CompoundField {
                    name: "a".into(),
                    ty: TypeDescriptor::FixedArray(Box::new(x_desc), 2),
                    offset: 0
                },
                CompoundField {
                    name: "b".into(),
                    ty: TypeDescriptor::FixedArray(Box::new(
                        TypeDescriptor::FixedArray(Box::new(a_desc), 4)), 32),
                    offset: 2 * 8
                },
            ],
            size: 2 * 8 + 4 * 32 * 16,
        });
        check_roundtrip!(C, c_desc);
    }
}