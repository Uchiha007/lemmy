use crate::PerformCrud;
use actix_web::web::Data;
use lemmy_api_common::{
  blocking,
  build_federated_instances,
  get_local_user_settings_view_from_jwt_opt,
  person::Register,
  site::*,
};
use lemmy_db_views::site_view::SiteView;
use lemmy_db_views_actor::{
  community_block_view::CommunityBlockView,
  community_follower_view::CommunityFollowerView,
  community_moderator_view::CommunityModeratorView,
  person_block_view::PersonBlockView,
  person_view::PersonViewSafe,
};
use lemmy_utils::{version, ConnectionId, LemmyError};
use lemmy_websocket::{messages::GetUsersOnline, LemmyContext};
use tracing::info;

#[async_trait::async_trait(?Send)]
impl PerformCrud for GetSite {
  type Response = GetSiteResponse;

  #[tracing::instrument(skip(context, websocket_id))]
  async fn perform(
    &self,
    context: &Data<LemmyContext>,
    websocket_id: Option<ConnectionId>,
  ) -> Result<GetSiteResponse, LemmyError> {
    let data: &GetSite = self;

    let site_view = match blocking(context.pool(), SiteView::read).await? {
      Ok(site_view) => Some(site_view),
      // If the site isn't created yet, check the setup
      Err(_) => {
        if let Some(setup) = context.settings().setup.as_ref() {
          let register = Register {
            username: setup.admin_username.to_owned(),
            email: setup.admin_email.clone().map(|s| s.into()),
            password: setup.admin_password.clone().into(),
            password_verify: setup.admin_password.clone().into(),
            show_nsfw: true,
            captcha_uuid: None,
            captcha_answer: None,
            honeypot: None,
          };
          let login_response = register.perform(context, websocket_id).await?;
          info!("Admin {} created", setup.admin_username);

          let create_site = CreateSite {
            name: setup.site_name.to_owned(),
            sidebar: setup.sidebar.to_owned(),
            description: setup.description.to_owned(),
            icon: setup.icon.to_owned(),
            banner: setup.banner.to_owned(),
            enable_downvotes: setup.enable_downvotes,
            open_registration: setup.open_registration,
            enable_nsfw: setup.enable_nsfw,
            community_creation_admin_only: setup.community_creation_admin_only,
            auth: login_response.jwt,
          };
          create_site.perform(context, websocket_id).await?;
          info!("Site {} created", setup.site_name);
          Some(blocking(context.pool(), SiteView::read).await??)
        } else {
          None
        }
      }
    };

    let mut admins = blocking(context.pool(), PersonViewSafe::admins).await??;

    // Make sure the site creator is the top admin
    if let Some(site_view) = site_view.to_owned() {
      let site_creator_id = site_view.creator.id;
      // TODO investigate why this is sometimes coming back null
      // Maybe user_.admin isn't being set to true?
      if let Some(creator_index) = admins.iter().position(|r| r.person.id == site_creator_id) {
        let creator_person = admins.remove(creator_index);
        admins.insert(0, creator_person);
      }
    }

    let banned = blocking(context.pool(), PersonViewSafe::banned).await??;

    let online = context
      .chat_server()
      .send(GetUsersOnline)
      .await
      .unwrap_or(1);

    // Build the local user
    let my_user = if let Some(local_user_view) = get_local_user_settings_view_from_jwt_opt(
      data.auth.as_ref(),
      context.pool(),
      context.secret(),
    )
    .await?
    {
      let person_id = local_user_view.person.id;
      let follows = blocking(context.pool(), move |conn| {
        CommunityFollowerView::for_person(conn, person_id)
      })
      .await?
      .map_err(LemmyError::from)
      .map_err(|e| e.with_message("system_err_login"))?;

      let person_id = local_user_view.person.id;
      let community_blocks = blocking(context.pool(), move |conn| {
        CommunityBlockView::for_person(conn, person_id)
      })
      .await?
      .map_err(LemmyError::from)
      .map_err(|e| e.with_message("system_err_login"))?;

      let person_id = local_user_view.person.id;
      let person_blocks = blocking(context.pool(), move |conn| {
        PersonBlockView::for_person(conn, person_id)
      })
      .await?
      .map_err(LemmyError::from)
      .map_err(|e| e.with_message("system_err_login"))?;

      let moderates = blocking(context.pool(), move |conn| {
        CommunityModeratorView::for_person(conn, person_id)
      })
      .await?
      .map_err(LemmyError::from)
      .map_err(|e| e.with_message("system_err_login"))?;

      Some(MyUserInfo {
        local_user_view,
        follows,
        moderates,
        community_blocks,
        person_blocks,
      })
    } else {
      None
    };

    let federated_instances = build_federated_instances(
      context.pool(),
      &context.settings().federation,
      &context.settings().hostname,
    )
    .await?;

    Ok(GetSiteResponse {
      site_view,
      admins,
      banned,
      online,
      version: version::VERSION.to_string(),
      my_user,
      federated_instances,
    })
  }
}
