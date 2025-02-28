use crate::{
  check_is_apub_id_valid,
  generate_outbox_url,
  objects::get_summary_from_string_or_source,
  protocol::{
    objects::{
      person::{Person, UserTypes},
      Endpoints,
    },
    ImageObject,
    Source,
  },
};
use chrono::NaiveDateTime;
use lemmy_api_common::blocking;
use lemmy_apub_lib::{
  object_id::ObjectId,
  traits::{ActorType, ApubObject},
  values::MediaTypeMarkdown,
  verify::verify_domains_match,
};
use lemmy_db_schema::{
  naive_now,
  source::person::{Person as DbPerson, PersonForm},
};
use lemmy_utils::{
  utils::{check_slurs, check_slurs_opt, convert_datetime, markdown_to_html},
  LemmyError,
};
use lemmy_websocket::LemmyContext;
use std::ops::Deref;
use url::Url;

#[derive(Clone, Debug, PartialEq)]
pub struct ApubPerson(DbPerson);

impl Deref for ApubPerson {
  type Target = DbPerson;
  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl From<DbPerson> for ApubPerson {
  fn from(p: DbPerson) -> Self {
    ApubPerson { 0: p }
  }
}

#[async_trait::async_trait(?Send)]
impl ApubObject for ApubPerson {
  type DataType = LemmyContext;
  type ApubType = Person;
  type TombstoneType = ();

  fn last_refreshed_at(&self) -> Option<NaiveDateTime> {
    Some(self.last_refreshed_at)
  }

  #[tracing::instrument(skip_all)]
  async fn read_from_apub_id(
    object_id: Url,
    context: &LemmyContext,
  ) -> Result<Option<Self>, LemmyError> {
    Ok(
      blocking(context.pool(), move |conn| {
        DbPerson::read_from_apub_id(conn, object_id)
      })
      .await??
      .map(Into::into),
    )
  }

  #[tracing::instrument(skip_all)]
  async fn delete(self, context: &LemmyContext) -> Result<(), LemmyError> {
    blocking(context.pool(), move |conn| {
      DbPerson::update_deleted(conn, self.id, true)
    })
    .await??;
    Ok(())
  }

  #[tracing::instrument(skip_all)]
  async fn into_apub(self, _pool: &LemmyContext) -> Result<Person, LemmyError> {
    let kind = if self.bot_account {
      UserTypes::Service
    } else {
      UserTypes::Person
    };
    let source = self.bio.clone().map(|bio| Source {
      content: bio,
      media_type: MediaTypeMarkdown::Markdown,
    });
    let icon = self.avatar.clone().map(ImageObject::new);
    let image = self.banner.clone().map(ImageObject::new);

    let person = Person {
      kind,
      id: ObjectId::new(self.actor_id.clone()),
      preferred_username: self.name.clone(),
      name: self.display_name.clone(),
      summary: self.bio.as_ref().map(|b| markdown_to_html(b)),
      source,
      icon,
      image,
      matrix_user_id: self.matrix_user_id.clone(),
      published: Some(convert_datetime(self.published)),
      outbox: generate_outbox_url(&self.actor_id)?.into(),
      endpoints: Endpoints {
        shared_inbox: self.shared_inbox_url.clone().map(|s| s.into()),
      },
      public_key: self.get_public_key()?,
      updated: self.updated.map(convert_datetime),
      unparsed: Default::default(),
      inbox: self.inbox_url.clone().into(),
    };
    Ok(person)
  }

  fn to_tombstone(&self) -> Result<(), LemmyError> {
    unimplemented!()
  }

  #[tracing::instrument(skip_all)]
  async fn verify(
    person: &Person,
    expected_domain: &Url,
    context: &LemmyContext,
    _request_counter: &mut i32,
  ) -> Result<(), LemmyError> {
    verify_domains_match(person.id.inner(), expected_domain)?;
    check_is_apub_id_valid(person.id.inner(), false, &context.settings())?;

    let slur_regex = &context.settings().slur_regex();
    check_slurs(&person.preferred_username, slur_regex)?;
    check_slurs_opt(&person.name, slur_regex)?;
    let bio = get_summary_from_string_or_source(&person.summary, &person.source);
    check_slurs_opt(&bio, slur_regex)?;
    Ok(())
  }

  #[tracing::instrument(skip_all)]
  async fn from_apub(
    person: Person,
    context: &LemmyContext,
    _request_counter: &mut i32,
  ) -> Result<ApubPerson, LemmyError> {
    let person_form = PersonForm {
      name: person.preferred_username,
      display_name: Some(person.name),
      banned: None,
      deleted: None,
      avatar: Some(person.icon.map(|i| i.url.into())),
      banner: Some(person.image.map(|i| i.url.into())),
      published: person.published.map(|u| u.naive_local()),
      updated: person.updated.map(|u| u.naive_local()),
      actor_id: Some(person.id.into()),
      bio: Some(get_summary_from_string_or_source(
        &person.summary,
        &person.source,
      )),
      local: Some(false),
      admin: Some(false),
      bot_account: Some(person.kind == UserTypes::Service),
      private_key: None,
      public_key: person.public_key.public_key_pem,
      last_refreshed_at: Some(naive_now()),
      inbox_url: Some(person.inbox.into()),
      shared_inbox_url: Some(person.endpoints.shared_inbox.map(|s| s.into())),
      matrix_user_id: Some(person.matrix_user_id),
    };
    let person = blocking(context.pool(), move |conn| {
      DbPerson::upsert(conn, &person_form)
    })
    .await??;
    Ok(person.into())
  }
}

impl ActorType for ApubPerson {
  fn actor_id(&self) -> Url {
    self.actor_id.to_owned().into()
  }

  fn public_key(&self) -> String {
    self.public_key.to_owned()
  }

  fn private_key(&self) -> Option<String> {
    self.private_key.to_owned()
  }

  fn inbox_url(&self) -> Url {
    self.inbox_url.clone().into()
  }

  fn shared_inbox_url(&self) -> Option<Url> {
    self.shared_inbox_url.clone().map(|s| s.into())
  }
}

#[cfg(test)]
pub(crate) mod tests {
  use super::*;
  use crate::objects::tests::{file_to_json_object, init_context};
  use lemmy_apub_lib::activity_queue::create_activity_queue;
  use lemmy_db_schema::traits::Crud;
  use serial_test::serial;

  pub(crate) async fn parse_lemmy_person(context: &LemmyContext) -> ApubPerson {
    let json = file_to_json_object("assets/lemmy/objects/person.json");
    let url = Url::parse("https://enterprise.lemmy.ml/u/picard").unwrap();
    let mut request_counter = 0;
    ApubPerson::verify(&json, &url, context, &mut request_counter)
      .await
      .unwrap();
    let person = ApubPerson::from_apub(json, context, &mut request_counter)
      .await
      .unwrap();
    assert_eq!(request_counter, 0);
    person
  }

  #[actix_rt::test]
  #[serial]
  async fn test_parse_lemmy_person() {
    let manager = create_activity_queue();
    let context = init_context(manager.queue_handle().clone());
    let person = parse_lemmy_person(&context).await;

    assert_eq!(person.display_name, Some("Jean-Luc Picard".to_string()));
    assert!(!person.local);
    assert_eq!(person.bio.as_ref().unwrap().len(), 39);

    DbPerson::delete(&*context.pool().get().unwrap(), person.id).unwrap();
  }

  #[actix_rt::test]
  #[serial]
  async fn test_parse_pleroma_person() {
    let manager = create_activity_queue();
    let context = init_context(manager.queue_handle().clone());
    let json = file_to_json_object("assets/pleroma/objects/person.json");
    let url = Url::parse("https://queer.hacktivis.me/users/lanodan").unwrap();
    let mut request_counter = 0;
    ApubPerson::verify(&json, &url, &context, &mut request_counter)
      .await
      .unwrap();
    let person = ApubPerson::from_apub(json, &context, &mut request_counter)
      .await
      .unwrap();

    assert_eq!(person.actor_id, url.into());
    assert_eq!(person.name, "lanodan");
    assert!(!person.local);
    assert_eq!(request_counter, 0);
    assert_eq!(person.bio.as_ref().unwrap().len(), 873);

    DbPerson::delete(&*context.pool().get().unwrap(), person.id).unwrap();
  }
}
