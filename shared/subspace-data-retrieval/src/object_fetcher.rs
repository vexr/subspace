//! Fetching objects stored in the archived history of Subspace Network.

use crate::object_fetcher::partial_object::{PartialObject, RawPieceData};
use crate::object_fetcher::segment_header::{
    MAX_SEGMENT_PADDING, max_segment_header_encoded_size, min_segment_header_encoded_size,
};
use crate::piece_fetcher::download_pieces;
use crate::piece_getter::PieceGetter;
use parity_scale_codec::{Compact, CompactLen, Decode};
use std::sync::Arc;
use subspace_archiving::archiver::SegmentItem;
use subspace_core_primitives::hashes::Blake3Hash;
use subspace_core_primitives::objects::{GlobalObject, GlobalObjectMapping};
use subspace_core_primitives::pieces::{Piece, PieceIndex, RawRecord};
use subspace_core_primitives::segments::{RecordedHistorySegment, SegmentIndex};
use tracing::{debug, trace, warn};

mod partial_object;
mod segment_header;
#[cfg(test)]
mod tests;

pub use segment_header::MAX_BLOCK_LENGTH;

/// The maximum object length the implementation in this module can reliably handle.
///
/// Currently objects are limited by the largest block size in the consensus chain, which is 5 MB.
/// But this implementation can retrieve all objects smaller than a segment (up to 124 MB). Some
/// objects between 124 MB and 248 MB are supported, if they span 2 segments (but not 3 segments).
/// But objects that large don't currently exist, so we use the lower limit to avoid potential
/// security and reliability issues.
///
/// The maximum object length excludes segment padding, and the parent segment header at the start
/// of the next segment.
//
// TODO: if the consensus chain supports larger block sizes, implement support for:
// - objects larger than 124 MB: reconstruct objects that span 3 or more segments, by
//   reconstructing each full segment
// - blocks larger than 1 GB: handle padding for blocks with encoded length prefixes that are
//   longer than 4 bytes, by increasing MAX_SEGMENT_PADDING
#[inline]
pub fn max_supported_object_length() -> usize {
    // segment - variable end padding - segment version variant - segment header item variant
    // - parent segment header - segment (block) item variant - block size - object size
    RecordedHistorySegment::SIZE
        - MAX_SEGMENT_PADDING
        - 1
        - 1
        - max_segment_header_encoded_size()
        - 1
        - MAX_ENCODED_LENGTH_SIZE * 2
}

/// The length of the compact encoding of `max_supported_object_length()`.
const MAX_ENCODED_LENGTH_SIZE: usize = 4;

/// Used to store the last piece downloaded in an object fetcher batch.
pub type LastPieceCache = (PieceIndex, Piece);

/// Object fetching errors.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Supplied piece index is not a source piece
    #[error("Piece index is not a source piece, object: {mapping:?}")]
    NotSourcePiece { mapping: GlobalObject },

    /// Supplied piece offset is too large
    #[error(
        "Piece offset is too large, must be less than {}, object: {mapping:?}",
        RawRecord::SIZE
    )]
    PieceOffsetTooLarge { mapping: GlobalObject },

    /// Object is too large error
    #[error(
        "Data length {data_length} exceeds maximum object size {max_object_len} \
         for object: {mapping:?}"
    )]
    ObjectTooLarge {
        data_length: usize,
        max_object_len: usize,
        mapping: GlobalObject,
    },

    /// Length prefix is too large error
    #[error(
        "Length prefix length {length_prefix_len} exceeds maximum object size {max_object_len} \
         for object: {mapping:?}"
    )]
    LengthPrefixTooLarge {
        length_prefix_len: usize,
        max_object_len: usize,
        mapping: GlobalObject,
    },

    /// Hash doesn't match data
    #[error("Incorrect data hash {data_hash:?} for {data_length} byte object: {mapping:?}")]
    InvalidDataHash {
        data_hash: Blake3Hash,
        data_length: usize,
        mapping: GlobalObject,
        // The hex-encoded object data, only used in tests
        #[cfg(test)]
        data: String,
    },

    /// Piece getter error
    #[error("Getting piece caused an error: {error}, object: {mapping:?}")]
    PieceGetterError {
        /// The original `anyhow::Error`, debug-printed as a string.
        /// This allows us to check errors for equality in tests.
        error: String,
        mapping: GlobalObject,
    },

    /// Piece getter couldn't find the piece
    #[error("Piece {piece_index:?} was not found by piece getter")]
    PieceNotFound { piece_index: PieceIndex },

    /// Supplied piece offset is inside the minimum segment header size
    #[error(
        "Piece offset is inside the segment header, min size of segment header: {}, object: {mapping:?}",
        min_segment_header_encoded_size()
    )]
    PieceOffsetInSegmentHeader { mapping: GlobalObject },

    /// Segment decoding error
    #[error("Segment {segment_index:?} data decoding error: {source:?}, object: {mapping:?}")]
    SegmentDecoding {
        source: parity_scale_codec::Error,
        segment_index: SegmentIndex,
        mapping: GlobalObject,
    },

    /// Unknown segment variant error
    #[error(
        "Decoding segment {segment_index:?} failed: unknown variant: {segment_variant}, \
         object: {mapping:?}"
    )]
    UnknownSegmentVariant {
        segment_variant: u8,
        segment_index: SegmentIndex,
        mapping: GlobalObject,
    },

    /// Unexpected segment item error
    #[error(
        "Segment {segment_index:?} has unexpected item, current progress: {segment_progress}, \
         object: {mapping:?}, item: {segment_item:?}"
    )]
    UnexpectedSegmentItem {
        segment_progress: usize,
        segment_index: SegmentIndex,
        segment_item: Box<SegmentItem>,
        mapping: GlobalObject,
    },

    /// Unexpected segment item variant error
    #[error(
        "Segment {segment_index:?} has unexpected item, current progress: {segment_progress}, \
         object: {mapping:?}, item: {segment_item_variant:?}, item size and data lengths: \
         {segment_item_lengths:?}"
    )]
    UnexpectedSegmentItemVariant {
        segment_progress: usize,
        segment_index: SegmentIndex,
        segment_item_variant: u8,
        segment_item_lengths: Option<(usize, usize)>,
        mapping: GlobalObject,
    },

    /// Object length couldn't be decoded after downloading two pieces
    #[error(
        "Invalid object: next source piece: {next_source_piece_index:?}, segment data length: \
         {segment_data_length:?}, object: {mapping:?}"
    )]
    InvalidObject {
        /// The next source piece index after the first two pieces
        next_source_piece_index: PieceIndex,
        /// The available object data in the current segment
        segment_data_length: Option<usize>,
        mapping: GlobalObject,
    },

    /// Object extends beyond block continuation, or the mapping is otherwise invalid
    #[error(
        "Invalid mapping: data length: {object_data_length:?}, next source piece: \
         {next_source_piece_index:?}, remaining_piece_count: {remaining_piece_count}, object: \
         {mapping:?}"
    )]
    InvalidMapping {
        /// The next source piece index, before we attempted concurrent downloads
        next_source_piece_index: PieceIndex,
        /// The number of pieces we concurrently downloaded
        remaining_piece_count: usize,
        /// The object data length, after the concurrent downloads
        object_data_length: usize,
        mapping: GlobalObject,
    },
}

/// Object fetcher for the Subspace DSN.
pub struct ObjectFetcher<PG>
where
    PG: PieceGetter + Send + Sync,
{
    /// The piece getter used to fetch pieces.
    piece_getter: Arc<PG>,

    /// The maximum number of data bytes we'll read for a single object.
    max_object_len: usize,
}

impl<PG> ObjectFetcher<PG>
where
    PG: PieceGetter + Send + Sync,
{
    /// Create a new object fetcher with the given configuration.
    ///
    /// `max_object_len` is the amount of data bytes we'll read for a single object before giving
    /// up and returning an error. In this implementation, it is limited to
    /// [`max_supported_object_length()`], which is much larger than the maximum consensus block
    /// size.
    pub fn new(piece_getter: Arc<PG>, mut max_object_len: usize) -> Self {
        if max_object_len > max_supported_object_length() {
            warn!(
                max_object_len,
                max_supported_object_length = ?max_supported_object_length(),
                "Object fetcher size limit exceeds maximum supported object size, \
                limiting to implementation-supported size"
            );

            max_object_len = max_supported_object_length();
        }

        Self {
            piece_getter,
            max_object_len,
        }
    }

    /// Assemble the objects in `mapping` by fetching necessary pieces using the piece getter, and
    /// putting the objects' bytes together.
    ///
    /// Checks the objects' hashes to make sure the correct bytes are returned.
    ///
    /// For efficiency, objects in a batch should be sorted by increasing piece index. Objects with
    /// the same piece index should be sorted by increasing offset. This allows the last piece to
    /// be re-used for the next object in the batch.
    ///
    /// Batches should be split if the gap between object piece indexes is 6 or more. Those objects
    /// can't share any pieces, because a maximum-sized object only uses 6 pieces.
    pub async fn fetch_objects(
        &self,
        mappings: GlobalObjectMapping,
    ) -> Result<Vec<Vec<u8>>, Error> {
        let mut objects = Vec::with_capacity(mappings.objects().len());
        let mut piece_cache = None;

        // TODO:
        // - keep the last downloaded piece until it's no longer needed
        // - document sorting mappings in piece index order
        for &mapping in mappings.objects() {
            let GlobalObject {
                piece_index,
                offset,
                ..
            } = mapping;

            // Validate parameters
            if !piece_index.is_source() {
                debug!(
                    ?mapping,
                    "Invalid piece index for object: must be a source piece",
                );

                // Parity pieces contain effectively random data, and can't be used to fetch
                // objects
                return Err(Error::NotSourcePiece { mapping });
            }

            // We could parse each segment header to do this check perfectly, but it's an edge
            // case, so we just do a best-effort check
            if piece_index.source_position() == 0
                && offset < min_segment_header_encoded_size() as u32
            {
                debug!(
                    ?mapping,
                    min_segment_header_encoded_size = ?min_segment_header_encoded_size(),
                    "Invalid offset for object: must not be inside the segment header",
                );

                return Err(Error::PieceOffsetInSegmentHeader { mapping });
            }

            if offset >= RawRecord::SIZE as u32 {
                debug!(
                    ?mapping,
                    RawRecord_SIZE = RawRecord::SIZE,
                    "Invalid piece offset for object: must be less than the size of a raw record",
                );

                return Err(Error::PieceOffsetTooLarge { mapping });
            }

            // All objects can be assembled from individual pieces, we handle segments by checking
            // all possible padding, and parsing and discarding segment headers.
            let data = self.fetch_object(mapping, &mut piece_cache).await?;

            objects.push(data);
        }

        Ok(objects)
    }

    /// Single object fetching and assembling.
    ///
    /// Each piece is initially turned into a PartialData struct. When there are enough pieces to
    /// calculate the object's length(s), those pieces are turned into a PartialObject struct.
    /// After that, each new piece becomes a PartialData (to track padding and segment headers),
    /// then gets added to the PartialObject.
    ///
    /// When the PartialObject has enough data for its shortest length, the data (and corresponding
    /// padding) is checked against the object hash. If that fails, we check more padding lengths,
    /// or fetch more data.
    //
    // TODO: return last downloaded piece from fetch_object() and pass them to the next fetch_object()
    async fn fetch_object(
        &self,
        mapping: GlobalObject,
        piece_cache: &mut Option<LastPieceCache>,
    ) -> Result<Vec<u8>, Error> {
        let GlobalObject {
            piece_index,
            offset,
            ..
        } = mapping;

        // The next piece we want to download, starting with piece at index `piece_index`
        let mut next_source_piece_index = piece_index;

        // The raw data we've read so far
        let mut raw_data = RawPieceData::new_for_first_piece(mapping);

        // Get pieces until we have enough data to calculate the object's length(s).
        // Objects with their length bytes at the end of a piece are a rare edge case.
        let piece = self
            .read_piece(next_source_piece_index, mapping, piece_cache)
            .await?;

        // Discard piece data before the offset.
        // If this is the first piece in a segment, this automatically skips the segment header.
        let piece_data = piece
            .record()
            .to_raw_record_chunks()
            .flatten()
            .skip(offset as usize)
            .copied()
            .collect::<Vec<u8>>();

        raw_data.add_piece_data(next_source_piece_index, piece_data, mapping)?;
        next_source_piece_index = next_source_piece_index.next_source_index();

        // Try to create a new partial object, this only works if we have enough data to find its length
        let mut partial_object = if let Some(partial_object) =
            PartialObject::new_with_padding(&raw_data, self.max_object_len, mapping)?
        {
            // We've used up this data, so just drop it
            std::mem::drop(raw_data);

            trace!(
                %next_source_piece_index,
                ?mapping,
                ?partial_object,
                "Successfully decoded partial object length from first piece",
            );

            partial_object
        } else {
            // Need the next piece to read the length of the object data
            trace!(
                %next_source_piece_index,
                ?mapping,
                ?raw_data,
                "Part of object length bytes are in next piece, fetching",
            );

            // Get the second piece for the object
            let piece = self
                .read_piece(next_source_piece_index, mapping, piece_cache)
                .await?;
            // We want all the piece data
            let piece_data = piece
                .record()
                .to_raw_record_chunks()
                .flatten()
                .copied()
                .collect::<Vec<u8>>();

            raw_data.add_piece_data(next_source_piece_index, piece_data, mapping)?;
            next_source_piece_index = next_source_piece_index.next_source_index();

            // We should have enough data to create a partial object now
            if let Some(partial_object) =
                PartialObject::new_with_padding(&raw_data, self.max_object_len, mapping)?
            {
                // We've used up this data, so just drop it
                std::mem::drop(raw_data);

                trace!(
                    %next_source_piece_index,
                    ?mapping,
                    ?partial_object,
                    "Successfully decoded partial object length from first two pieces",
                );

                partial_object
            } else {
                // There's something wrong with the mapping, because we can't decode the object's
                // length after two pieces
                return Err(Error::InvalidObject {
                    next_source_piece_index,
                    segment_data_length: raw_data.segment_data_length(),
                    mapping,
                });
            }
        };

        // We might already have the whole object, let's check before downloading more pieces
        if let Some(data) = partial_object.try_reconstruct_object(mapping)? {
            return Ok(data);
        }

        // Read more pieces until we have enough data for all possible object lengths.
        //
        // Adding padding can change the size of the object up to 256x. But the maximum object size
        // is 6 pieces, so we get better latency by downloading any pieces that could be needed at
        // the same time. (Larger objects have already been rejected during length decoding.)
        let remaining_piece_count = partial_object
            .max_remaining_download_length()
            .div_ceil(RawRecord::SIZE);

        if remaining_piece_count > 0 {
            let remaining_piece_indexes = (next_source_piece_index..)
                .filter(|i| i.is_source())
                .take(remaining_piece_count)
                .collect::<Arc<[PieceIndex]>>();
            // TODO: turn this into a concurrent stream, which cancels piece downloads if they aren't
            // needed
            let pieces = self
                .read_pieces(remaining_piece_indexes.clone(), mapping, piece_cache)
                .await?
                .into_iter()
                .zip(remaining_piece_indexes.iter().copied())
                .map(|(piece, piece_index)| {
                    (
                        piece_index,
                        piece
                            .record()
                            .to_raw_record_chunks()
                            .flatten()
                            .copied()
                            .collect::<Vec<u8>>(),
                    )
                });

            for (piece_index, piece_data) in pieces {
                let mut new_data = RawPieceData::new_for_next_piece(
                    partial_object.max_remaining_download_length(),
                    piece_index,
                );
                new_data.add_piece_data(piece_index, piece_data, mapping)?;
                partial_object.add_piece_data_with_padding(new_data);

                // We might already have the whole object, let's check before decoding more pieces
                if let Some(data) = partial_object.try_reconstruct_object(mapping)? {
                    return Ok(data);
                }
            }
        }

        // If the mapping is invalid, we can try to read beyond the downloaded pieces.
        // Specifically, if a cross-segment object's offset is wrong, we can try to read beyond the
        // block continuation at the start of the second segment.
        Err(Error::InvalidMapping {
            next_source_piece_index,
            remaining_piece_count,
            object_data_length: partial_object.fetched_data_length(),
            mapping,
        })
    }

    /// Concurrently read multiple pieces, and return them in the supplied order.
    ///
    /// The mapping is only used for error reporting.
    async fn read_pieces(
        &self,
        piece_indexes: Arc<[PieceIndex]>,
        mapping: GlobalObject,
        piece_cache: &mut Option<LastPieceCache>,
    ) -> Result<Vec<Piece>, Error> {
        download_pieces(
            piece_indexes.clone(),
            &piece_cache.clone().with_fallback(self.piece_getter.clone()),
        )
        .await
        .inspect(|pieces| {
            if let (Some(piece_index), Some(piece)) = (piece_indexes.last(), pieces.last()) {
                *piece_cache = Some((*piece_index, piece.clone()))
            }
        })
        .map_err(|source| {
            debug!(
                ?piece_indexes,
                error = ?source,
                ?mapping,
                "Error fetching pieces during object assembling"
            );

            Error::PieceGetterError {
                error: format!("{source:?}"),
                mapping,
            }
        })
    }

    /// Read and return a single piece.
    ///
    /// The mapping is only used for error reporting.
    async fn read_piece(
        &self,
        piece_index: PieceIndex,
        mapping: GlobalObject,
        piece_cache: &mut Option<LastPieceCache>,
    ) -> Result<Piece, Error> {
        let piece_indexes = Arc::<[PieceIndex]>::from(vec![piece_index]);
        download_pieces(
            piece_indexes.clone(),
            &piece_cache.clone().with_fallback(self.piece_getter.clone()),
        )
        .await
        .inspect(|pieces| *piece_cache = Some((piece_index, pieces[0].clone())))
        .map(|pieces| {
            pieces
                .first()
                .expect("download_pieces always returns exact pieces or error")
                .clone()
        })
        .map_err(|source| {
            debug!(
                %piece_index,
                error = ?source,
                ?mapping,
                "Error fetching piece during object assembling"
            );

            Error::PieceGetterError {
                error: format!("{source:?}"),
                mapping,
            }
        })
    }
}

/// Validate and decode the encoded length of `data`, including the encoded length bytes.
/// `data` may be incomplete.
///
/// Returns `Ok(Some((data_length_encoded_length, data_length)))` if the length is valid,
/// `Ok(None)` if there aren't enough bytes to decode the length, otherwise an error.
///
/// The mapping is only used for error reporting.
fn decode_data_length(
    mut data: &[u8],
    max_object_len: usize,
    mapping: GlobalObject,
) -> Result<Option<(usize, usize)>, Error> {
    let data_length = match Compact::<u32>::decode(&mut data) {
        Ok(Compact(data_length)) => {
            let data_length = data_length as usize;
            if data_length > max_object_len {
                debug!(
                    data_length,
                    max_object_len,
                    ?mapping,
                    "Data length exceeds object size limit for object fetcher"
                );

                return Err(Error::ObjectTooLarge {
                    data_length,
                    max_object_len,
                    mapping,
                });
            }

            data_length
        }
        Err(err) => {
            // Parity doesn't have an easily matched error enum, and all bit sequences are
            // valid compact encodings. So we assume we don't have enough bytes to decode the
            // length, unless we already have enough bytes to decode the maximum length.
            if data.len() >= Compact::<u32>::compact_len(&(max_object_len as u32)) {
                debug!(
                    length_prefix_len = data.len(),
                    max_object_len,
                    ?mapping,
                    "Length prefix exceeds object size limit for object fetcher"
                );

                return Err(Error::LengthPrefixTooLarge {
                    length_prefix_len: data.len(),
                    max_object_len,
                    mapping,
                });
            }

            debug!(
                ?err,
                ?mapping,
                "Not enough bytes to decode data length for object"
            );

            return Ok(None);
        }
    };

    let data_length_encoded_length = Compact::<u32>::compact_len(&(data_length as u32));

    trace!(
        data_length,
        data_length_encoded_length,
        ?mapping,
        "Decoded data length for object"
    );

    Ok(Some((data_length_encoded_length, data_length)))
}
