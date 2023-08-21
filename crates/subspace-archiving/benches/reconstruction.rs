use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rand::{thread_rng, Rng};
use subspace_archiving::piece_reconstructor::PiecesReconstructor;
use subspace_archiving::archiver::Archiver;
use subspace_core_primitives::{RecordedHistorySegment, Piece};
use subspace_core_primitives::crypto::kzg;
use subspace_core_primitives::crypto::kzg::Kzg;
use subspace_core_primitives::objects::BlockObjectMapping;


fn criterion_benchmark(c: &mut Criterion) {
    let kzg = Kzg::new(kzg::embedded_kzg_settings());
    let mut archiver = Archiver::new(kzg.clone()).unwrap();
    // Block that fits into the segment fully
    let mut block = vec![0u8; RecordedHistorySegment::SIZE];
    thread_rng().fill(block.as_mut_slice());

    let archived_segments = archiver.add_block(block, BlockObjectMapping::default(), true);


    let mut maybe_pieces: Vec<Option<Piece>> = archived_segments.first().unwrap().pieces.iter().map(Piece::from).map(Some).collect();

    // Remove some pieces from the vector
    maybe_pieces
    .iter_mut()
    .skip(100)
    .take(100)
    .for_each(|piece| {
        piece.take();
    });

    let reconstructor = PiecesReconstructor::new(kzg).unwrap();

    c.bench_function("segment-reconstruction", |b|{
     b.iter(|| {
            reconstructor.clone()
            .reconstruct_segment(black_box(&maybe_pieces))
            .unwrap();
        })
    });

}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
