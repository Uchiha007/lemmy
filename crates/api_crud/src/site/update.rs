use crate::PerformCrud;
use actix_web::web::Data;
use lemmy_api_common::{
  blocking,
  get_local_user_view_from_jwt,
  is_admin,
  site::{EditSite, SiteResponse},
  site_description_length_check,
};
use lemmy_db_schema::{
  diesel_option_overwrite,
  diesel_option_overwrite_to_url,
  naive_now,
  source::site::{Site, SiteForm},
  traits::Crud,
};
use lemmy_db_views::site_view::SiteView;
use lemmy_utils::{utils::check_slurs_opt, ConnectionId, LemmyError};
use lemmy_websocket::{messages::SendAllMessage, LemmyContext, UserOperationCrud};

#[async_trait::async_trait(?Send)]
impl PerformCrud for EditSite {
  type Response = SiteResponse;

  #[tracing::instrument(skip(context, websocket_id))]
  async fn perform(
    &self,
    context: &Data<LemmyContext>,
    websocket_id: Option<ConnectionId>,
  ) -> Result<SiteResponse, LemmyError> {
    let data: &EditSite = self;
    let local_user_view =
      get_local_user_view_from_jwt(&data.auth, context.pool(), context.secret()).await?;

    check_slurs_opt(&data.name, &context.settings().slur_regex())?;
    check_slurs_opt(&data.description, &context.settings().slur_regex())?;

    // Make sure user is an admin
    is_admin(&local_user_view)?;

    let found_site = blocking(context.pool(), Site::read_simple).await??;

    let sidebar = diesel_option_overwrite(&data.sidebar);
    let description = diesel_option_overwrite(&data.description);
    let icon = diesel_option_overwrite_to_url(&data.icon)?;
    let banner = diesel_option_overwrite_to_url(&data.banner)?;

    if let Some(Some(desc)) = &description {
      site_description_length_check(desc)?;
    }

    let site_form = SiteForm {
      creator_id: found_site.creator_id,
      name: data.name.to_owned().unwrap_or(found_site.name),
      sidebar,
      description,
      icon,
      banner,
      updated: Some(naive_now()),
      enable_downvotes: data.enable_downvotes,
      open_registration: data.open_registration,
      enable_nsfw: data.enable_nsfw,
      community_creation_admin_only: data.community_creation_admin_only,
    };

    let update_site = move |conn: &'_ _| Site::update(conn, 1, &site_form);
    blocking(context.pool(), update_site)
      .await?
      .map_err(LemmyError::from)
      .map_err(|e| e.with_message("couldnt_update_site"))?;

    let site_view = blocking(context.pool(), SiteView::read).await??;

    let res = SiteResponse { site_view };

    context.chat_server().do_send(SendAllMessage {
      op: UserOperationCrud::EditSite,
      response: res.clone(),
      websocket_id,
    });

    Ok(res)
  }
}
