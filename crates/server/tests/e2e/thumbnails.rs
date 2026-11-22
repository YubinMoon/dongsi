//! 방송 목록 썸네일(D-076): 방송자가 올리기 → 다른 클라이언트가 목록의 그림 번호를 보고 받기,
//! 마지막 한 장만 남음, 비밀번호 방송은 거절·미배포, 크기 상한, 너무 잦은 올리기, 방송이 끝나면 없어짐.

use std::time::Duration;

use dongsi_core::proto::{
    ClientMsg, ErrorCode, MAX_THUMBNAIL_BYTES, PublishInfo, ServerMsg, decode_thumbnail,
    encode_thumbnail,
};
use dongsi_core::{InstallId, VideoCodec};

use crate::client::{Client, find};
use crate::target::Target;

/// 서버가 같은 방송의 썸네일을 다시 받기까지의 간격(signaling.rs `MIN_THUMBNAIL_INTERVAL`)보다 조금 길게.
const REUPLOAD_WAIT: Duration = Duration::from_millis(10_500);

fn info() -> PublishInfo {
    PublishInfo {
        codec: VideoCodec::H264,
        width: 1280,
        height: 720,
        fps: 30,
        has_audio: true,
    }
}

/// 시험용 JPEG: SOI 표시(FF D8 FF) 뒤를 `fill`로 채운 `len`바이트. 서버는 앞 3바이트와 크기만 본다.
fn fake_jpeg(len: usize, fill: u8) -> Vec<u8> {
    let mut v = vec![fill; len.max(3)];
    v[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
    v
}

/// 방송 목록에서 `id` 방송의 그림 번호가 `pred`를 만족할 때까지 기다리고 그 번호를 돌려준다.
async fn wait_thumb(
    c: &mut Client,
    what: &str,
    id: InstallId,
    mut pred: impl FnMut(Option<u64>) -> bool,
) -> Option<u64> {
    c.wait_list(what, |l| find(l, id).is_some_and(|b| pred(b.thumbnail)))
        .await;
    find(&c.list, id).and_then(|b| b.thumbnail)
}

/// `GetThumbnail`을 보내고 답(번호, base64) 또는 오류 코드를 기다린다.
async fn get(c: &mut Client, broadcaster: InstallId) -> Result<(u64, String), ErrorCode> {
    let req = c.next_req();
    c.send(ClientMsg::GetThumbnail { req, broadcaster });
    c.wait_for("썸네일 응답", |m| match m {
        ServerMsg::Thumbnail {
            req: r,
            broadcaster: b,
            id,
            jpeg,
        } if *r == req && *b == broadcaster => Some(Ok((*id, jpeg.clone()))),
        ServerMsg::Error {
            req: Some(r), code, ..
        } if *r == req => Some(Err(*code)),
        _ => None,
    })
    .await
}

/// `req` 없는 오류(썸네일 올리기 거절)를 기다린다.
async fn upload_error(c: &mut Client) -> ErrorCode {
    c.wait_for("썸네일 거절", |m| match m {
        ServerMsg::Error {
            req: None, code, ..
        } => Some(*code),
        _ => None,
    })
    .await
}

fn upload(c: &Client, jpeg: &[u8]) {
    c.send(ClientMsg::Thumbnail {
        jpeg: encode_thumbnail(jpeg),
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thumbnails() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "썸네일 방송자").await;
    let id = b.id;
    let _p = b.publish(info(), None).await.unwrap();
    let mut v = Client::connect(&t, "썸네일 시청자").await;
    // 처음에는 그림이 없다.
    assert_eq!(
        wait_thumb(&mut v, "방송이 보임", id, |_| true).await,
        None,
        "올리기 전인데 그림 번호가 있습니다"
    );
    assert_eq!(get(&mut v, id).await.err(), Some(ErrorCode::NotFound));

    // 형식·크기가 틀린 그림은 받지 않는다(저장하지 않으므로 다음 올리기는 횟수 제한에 걸리지 않는다).
    upload(&b, &fake_jpeg(MAX_THUMBNAIL_BYTES + 1, 1));
    assert_eq!(upload_error(&mut b).await, ErrorCode::BadRequest);
    upload(&b, b"\x89PNG\r\n\x1a\n not a jpeg");
    assert_eq!(upload_error(&mut b).await, ErrorCode::BadRequest);
    b.send(ClientMsg::Thumbnail {
        jpeg: "base64가 아님".into(),
    });
    assert_eq!(upload_error(&mut b).await, ErrorCode::BadRequest);
    // 방송하지 않는 접속이 올리면 NotFound(연결은 그대로).
    upload(&v, &fake_jpeg(100, 2));
    assert_eq!(upload_error(&mut v).await, ErrorCode::NotFound);

    // 올리기(상한 크기 딱 맞춤) → 목록에는 번호만, 그림은 요청한 사람만 받는다.
    let first = fake_jpeg(MAX_THUMBNAIL_BYTES, 3);
    upload(&b, &first);
    let id1 = wait_thumb(&mut v, "그림 번호가 생김", id, |n| n.is_some())
        .await
        .unwrap();
    let pushed = v
        .try_wait_for(Duration::from_millis(300), |m| {
            matches!(m, ServerMsg::Thumbnail { .. }).then_some(())
        })
        .await;
    assert!(pushed.is_err(), "요청하지 않았는데 그림이 왔습니다");
    let (got_id, jpeg) = get(&mut v, id).await.expect("썸네일을 받지 못했습니다");
    assert_eq!(got_id, id1);
    assert_eq!(decode_thumbnail(&jpeg).as_deref(), Some(&first[..]));

    // 너무 잦은 올리기는 버린다(앞 그림이 남는다).
    upload(&b, &fake_jpeg(200, 4));
    assert_eq!(upload_error(&mut b).await, ErrorCode::RateLimited);
    assert_eq!(get(&mut v, id).await.map(|(n, _)| n), Ok(id1));

    // 간격이 지나면 새 그림이 앞 그림을 바꾼다(방송마다 한 장만).
    tokio::time::sleep(REUPLOAD_WAIT).await;
    let second = fake_jpeg(5_000, 5);
    upload(&b, &second);
    let id2 = wait_thumb(&mut v, "그림 번호가 바뀜", id, |n| {
        n.is_some_and(|n| n != id1)
    })
    .await
    .unwrap();
    let (got_id, jpeg) = get(&mut v, id).await.unwrap();
    assert_eq!(got_id, id2);
    assert_eq!(decode_thumbnail(&jpeg).as_deref(), Some(&second[..]));

    // 방송이 끝나면 그림도 없어진다. 같은 방송자가 다시 방송해도 앞 그림은 없다.
    b.send(ClientMsg::Unpublish);
    v.wait_list("방송이 사라짐", |l| find(l, id).is_none())
        .await;
    assert_eq!(get(&mut v, id).await.err(), Some(ErrorCode::NotFound));
    let _p2 = b.publish(info(), None).await.unwrap();
    assert_eq!(
        wait_thumb(&mut v, "다시 방송", id, |_| true).await,
        None,
        "다시 시작한 방송에 앞 방송의 그림이 남았습니다"
    );
    assert_eq!(get(&mut v, id).await.err(), Some(ErrorCode::NotFound));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn locked_broadcast_thumbnails() {
    let t = Target::shared_with(1).await;
    let mut b = Client::connect(&t, "잠긴 썸네일 방송자").await;
    let id = b.id;
    let _p = b.publish(info(), Some("비밀")).await.unwrap();
    let mut v = Client::connect(&t, "잠긴 썸네일 시청자").await;
    v.wait_list("잠긴 방송이 보임", |l| {
        find(l, id).is_some_and(|x| x.locked)
    })
    .await;

    // 비밀번호 방송의 썸네일은 받지 않고, 목록에 번호도 싣지 않고, 내주지도 않는다.
    upload(&b, &fake_jpeg(1_000, 6));
    assert_eq!(upload_error(&mut b).await, ErrorCode::NotAllowed);
    // 거절 뒤에 목록이 바뀌어도(시청자 수) 그림 번호는 없다.
    let w = v.watch(id, Some("비밀")).await.expect("시청 실패");
    let item = {
        v.wait_list("시청자 수가 바뀜", |l| {
            find(l, id).is_some_and(|x| x.viewers == 1)
        })
        .await;
        find(&v.list, id).cloned().unwrap()
    };
    assert_eq!(item.thumbnail, None);
    assert_eq!(get(&mut v, id).await.err(), Some(ErrorCode::NotFound));
    v.send(ClientMsg::StopWatch { session: w.session });
}
