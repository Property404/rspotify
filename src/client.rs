//! Client to Spotify API endpoint
// 3rd-part library
use chrono::prelude::*;
use log::{error, trace};
use reqwest::header::{HeaderMap, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, Method, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::map::Map;
use serde_json::{json, Value};
use thiserror::Error;

// Built-in battery
use std::borrow::Cow;
use std::collections::HashMap;
use std::string::String;

use super::model::album::{FullAlbum, FullAlbums, PageSimpliedAlbums, SavedAlbum, SimplifiedAlbum};
use super::model::artist::{CursorPageFullArtists, FullArtist, FullArtists};
use super::model::audio::{AudioAnalysis, AudioFeatures, AudioFeaturesPayload};
use super::model::category::PageCategory;
use super::model::context::{CurrentlyPlaybackContext, CurrentlyPlayingContext};
use super::model::cud_result::CUDResult;
use super::model::device::DevicePayload;
use super::model::page::{CursorBasedPage, Page};
use super::model::playing::{PlayHistory, Playing};
use super::model::playlist::{FeaturedPlaylists, FullPlaylist, PlaylistTrack, SimplifiedPlaylist};
use super::model::recommend::Recommendations;
use super::model::search::SearchResult;
use super::model::show::{
    FullEpisode, FullShow, SeveralEpisodes, SeversalSimplifiedShows, Show, SimplifiedEpisode,
};
use super::model::track::{FullTrack, FullTracks, SavedTrack, SimplifiedTrack};
use super::model::user::{PrivateUser, PublicUser};
use super::oauth2::SpotifyClientCredentials;
use super::senum::{
    AdditionalType, AlbumType, Country, IncludeExternal, RepeatState, SearchType, TimeRange, Type,
};

/// Possible errors returned from the `rspotify` client.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("request unauthorized")]
    Unauthorized,
    #[error("exceeded request limit")]
    RateLimited(Option<usize>),
    #[error("spotify error: {0}")]
    Api(#[from] ApiError),
    #[error("json parse error: {0}")]
    ParseJSON(#[from] serde_json::Error),
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("status code: {0}")]
    StatusCode(StatusCode),
}

impl ClientError {
    async fn from_response(response: Response) -> Self {
        match response.status() {
            StatusCode::UNAUTHORIZED => Self::Unauthorized,
            StatusCode::TOO_MANY_REQUESTS => Self::RateLimited(
                response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|header| header.to_str().ok())
                    .and_then(|duration| duration.parse().ok()),
            ),
            status @ StatusCode::FORBIDDEN | status @ StatusCode::NOT_FOUND => response
                .json::<ApiError>()
                .await
                .map(Into::into)
                .unwrap_or_else(|_| status.into()),
            status => status.into(),
        }
    }
}

impl From<StatusCode> for ClientError {
    fn from(code: StatusCode) -> Self {
        Self::StatusCode(code)
    }
}

/// Matches errors that are returned from the Spotfiy
/// API as part of the JSON response object.
#[derive(Debug, Error, Deserialize)]
pub enum ApiError {
    /// See https://developer.spotify.com/documentation/web-api/reference/object-model/#error-object
    #[error("{status}: {message}")]
    #[serde(alias = "error")]
    Regular { status: u16, message: String },

    /// See https://developer.spotify.com/documentation/web-api/reference/object-model/#player-error-object
    #[error("{status} ({reason}): {message}")]
    #[serde(alias = "error")]
    Player {
        status: u16,
        message: String,
        reason: String,
    },
}

type ClientResult<T> = Result<T, ClientError>;

/// Spotify API object
#[derive(Debug, Clone)]
pub struct Spotify {
    client: Client,
    pub prefix: String,
    pub access_token: Option<String>,
    pub client_credentials_manager: Option<SpotifyClientCredentials>,
}
impl Spotify {
    //! If you want to check examples of all API endpoint, you could check the
    //! [examples](https://github.com/samrayleung/rspotify/tree/master/examples) in github
    pub fn default() -> Spotify {
        Spotify {
            client: Client::new(),
            prefix: "https://api.spotify.com/v1/".to_owned(),
            access_token: None,
            client_credentials_manager: None,
        }
    }

    // pub fn prefix(mut self, prefix: &str) -> Spotify {
    pub fn prefix(mut self, prefix: &str) -> Spotify {
        self.prefix = prefix.to_owned();
        self
    }

    pub fn access_token(mut self, access_token: &str) -> Spotify {
        self.access_token = Some(access_token.to_owned());
        self
    }

    pub fn client_credentials_manager(
        mut self,
        client_credential_manager: SpotifyClientCredentials,
    ) -> Spotify {
        self.client_credentials_manager = Some(client_credential_manager);
        self
    }

    pub fn build(self) -> Spotify {
        if self.access_token.is_none() && self.client_credentials_manager.is_none() {
            panic!("access_token and client_credentials_manager are none!!!");
        }
        self
    }

    async fn auth_headers(&self) -> String {
        let token = match self.access_token {
            Some(ref token) => token.to_owned(),
            None => match self.client_credentials_manager {
                Some(ref client_credentials_manager) => {
                    client_credentials_manager.get_access_token().await
                }
                None => panic!("client credentials manager is none"),
            },
        };
        "Bearer ".to_owned() + &token
    }

    async fn internal_call<T: Serialize + ?Sized>(
        &self,
        method: Method,
        url: &str,
        payload: &T,
    ) -> ClientResult<String> {
        let mut url: Cow<str> = url.into();
        if !url.starts_with("http") {
            url = ["https://api.spotify.com/v1/", &url].concat().into();
        }

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, self.auth_headers().await.parse().unwrap());
        headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());

        let response = {
            let builder = self
                .client
                .request(method.clone(), &url.into_owned())
                .headers(headers);
            let builder = match method {
                Method::GET => builder.query(payload),
                Method::POST | Method::PUT | Method::DELETE => builder.json(payload),
                // Method: Options, Head, Trace haven't implemented in `rspotify` yet, just leave it alone.
                _ => builder,
            };

            builder.send().await.map_err(ClientError::from)?
        };

        if response.status().is_success() {
            response.text().await.map_err(Into::into)
        } else {
            Err(ClientError::from_response(response).await)
        }
    }
    /// Send get request
    async fn get(&self, url: &str, params: &mut HashMap<String, String>) -> ClientResult<String> {
        self.internal_call(Method::GET, url, params).await
    }

    /// Send post request
    async fn post(&self, url: &str, payload: &Value) -> ClientResult<String> {
        self.internal_call(Method::POST, url, payload).await
    }
    /// Send put request
    async fn put(&self, url: &str, payload: &Value) -> ClientResult<String> {
        self.internal_call(Method::PUT, url, payload).await
    }
    /// send delete request
    async fn delete(&self, url: &str, payload: &Value) -> ClientResult<String> {
        self.internal_call(Method::DELETE, url, payload).await
    }

    /// [get-track](https://developer.spotify.com/web-api/get-track/)
    /// returns a single track given the track's ID, URI or URL
    /// Parameters:
    /// - track_id - a spotify URI, URL or ID
    pub async fn track(&self, track_id: &str) -> ClientResult<FullTrack> {
        let trid = self.get_id(Type::Track, track_id);
        let url = format!("tracks/{}", trid);
        let result = self.get(&url, &mut HashMap::new()).await?;
        self.convert_result::<FullTrack>(&result)
    }

    /// [get-several-tracks](https://developer.spotify.com/web-api/get-several-tracks/)
    /// returns a list of tracks given a list of track IDs, URIs, or URLs
    /// Parameters:
    /// - track_ids - a list of spotify URIs, URLs or IDs
    /// - market - an ISO 3166-1 alpha-2 country code.
    pub async fn tracks(
        &self,
        track_ids: impl IntoIterator<Item = &str>,
        market: Option<Country>,
    ) -> ClientResult<FullTracks> {
        let mut ids: Vec<String> = vec![];
        for track_id in track_ids {
            ids.push(self.get_id(Type::Track, track_id));
        }
        let url = format!("tracks/?ids={}", ids.join(","));
        let mut params: HashMap<String, String> = HashMap::new();
        if let Some(_market) = market {
            params.insert("market".to_owned(), _market.as_str().to_owned());
        }
        trace!("{:?}", &url);
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<FullTracks>(&result)
    }

    /// [get-artist](https://developer.spotify.com/web-api/get-artist/)
    /// returns a single artist given the artist's ID, URI or URL
    /// Parameters:
    /// - artist_id - an artist ID, URI or URL
    pub async fn artist(&self, artist_id: &str) -> ClientResult<FullArtist> {
        let trid = self.get_id(Type::Artist, artist_id);
        let url = format!("artists/{}", trid);
        let result = self.get(&url, &mut HashMap::new()).await?;
        self.convert_result::<FullArtist>(&result)
    }

    /// [get-several-artists](https://developer.spotify.com/web-api/get-several-artists/)
    /// returns a list of artists given the artist IDs, URIs, or URLs
    /// Parameters:
    /// - artist_ids - a list of  artist IDs, URIs or URLs
    pub async fn artists(
        &self,
        artist_ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<FullArtists> {
        let mut ids: Vec<String> = vec![];
        for artist_id in artist_ids {
            ids.push(self.get_id(Type::Artist, &artist_id));
        }
        let url = format!("artists/?ids={}", ids.join(","));
        let result = self.get(&url, &mut HashMap::new()).await?;
        self.convert_result::<FullArtists>(&result)
    }

    /// [get-artists-albums](https://developer.spotify.com/web-api/get-artists-albums/)
    /// Get Spotify catalog information about an artist's albums
    /// - artist_id - the artist ID, URI or URL
    /// - album_type - 'album', 'single', 'appears_on', 'compilation'
    /// - country - limit the response to one particular country.
    /// - limit  - the number of albums to return
    /// - offset - the index of the first album to return
    pub async fn artist_albums(
        &self,
        artist_id: &str,
        album_type: Option<AlbumType>,
        country: Option<Country>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> ClientResult<Page<SimplifiedAlbum>> {
        let mut params: HashMap<String, String> = HashMap::new();
        if let Some(_limit) = limit {
            params.insert("limit".to_owned(), _limit.to_string());
        }
        if let Some(_album_type) = album_type {
            params.insert("album_type".to_owned(), _album_type.as_str().to_owned());
        }
        if let Some(_offset) = offset {
            params.insert("offset".to_owned(), _offset.to_string());
        }
        if let Some(_country) = country {
            params.insert("country".to_owned(), _country.as_str().to_owned());
        }
        let trid = self.get_id(Type::Artist, artist_id);
        let url = format!("artists/{}/albums", trid);
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<Page<SimplifiedAlbum>>(&result)
    }

    /// [get artists to tracks](https://developer.spotify.com/web-api/get-artists-top-tracks/)
    /// Get Spotify catalog information about an artist's top 10 tracks by country.
    /// Parameters:
    ///        - artist_id - the artist ID, URI or URL
    ///        - country - limit the response to one particular country.
    pub async fn artist_top_tracks<T: Into<Option<Country>>>(
        &self,
        artist_id: &str,
        country: T,
    ) -> ClientResult<FullTracks> {
        let mut params: HashMap<String, String> = HashMap::new();
        let country = country
            .into()
            .unwrap_or(Country::UnitedStates)
            .as_str()
            .to_string();
        params.insert("country".to_owned(), country);
        let trid = self.get_id(Type::Artist, artist_id);
        let url = format!("artists/{}/top-tracks", trid);

        let result = self.get(&url, &mut params).await?;
        self.convert_result::<FullTracks>(&result)
    }

    /// [get related artists](https://developer.spotify.com/web-api/get-related-artists/)
    /// Get Spotify catalog information about artists similar to an
    /// identified artist. Similarity is based on analysis of the
    /// Spotify community's listening history.
    /// Parameters:
    /// - artist_id - the artist ID, URI or URL
    pub async fn artist_related_artists(&self, artist_id: &str) -> ClientResult<FullArtists> {
        let trid = self.get_id(Type::Artist, artist_id);
        let url = format!("artists/{}/related-artists", trid);
        let result = self.get(&url, &mut HashMap::new()).await?;
        self.convert_result::<FullArtists>(&result)
    }

    /// [get album](https://developer.spotify.com/web-api/get-album/)
    /// returns a single album given the album's ID, URIs or URL
    /// Parameters:
    /// - album_id - the album ID, URI or URL
    pub async fn album(&self, album_id: &str) -> ClientResult<FullAlbum> {
        let trid = self.get_id(Type::Album, album_id);
        let url = format!("albums/{}", trid);
        let result = self.get(&url, &mut HashMap::new()).await?;
        self.convert_result::<FullAlbum>(&result)
    }

    /// [get several albums](https://developer.spotify.com/web-api/get-several-albums/)
    /// returns a list of albums given the album IDs, URIs, or URLs
    /// Parameters:
    /// - albums_ids - a list of  album IDs, URIs or URLs
    pub async fn albums(&self, album_ids: Vec<String>) -> ClientResult<FullAlbums> {
        let mut ids: Vec<String> = vec![];
        for album_id in album_ids {
            ids.push(self.get_id(Type::Album, &album_id));
        }
        let url = format!("albums/?ids={}", ids.join(","));
        let result = self.get(&url, &mut HashMap::new()).await?;
        self.convert_result::<FullAlbums>(&result)
    }

    /// [search for items](https://developer.spotify.com/web-api/search-item/)
    /// Search for an Item
    /// Get Spotify catalog information about artists, albums, tracks or
    /// playlists that match a keyword string.
    /// Parameters:
    /// - q - the search query
    /// - limit  - the number of items to return
    /// - offset - the index of the first item to return
    /// - type - the type of item to return. One of 'artist', 'album', 'track',
    ///  'playlist', 'show' or 'episode'
    /// - market - An ISO 3166-1 alpha-2 country code or the string from_token.
    /// - include_external: Optional.Possible values: audio. If include_external=audio is specified the response will include any relevant audio content that is hosted externally.  
    pub async fn search<L: Into<Option<u32>>, O: Into<Option<u32>>>(
        &self,
        q: &str,
        _type: SearchType,
        limit: L,
        offset: O,
        market: Option<Country>,
        include_external: Option<IncludeExternal>,
    ) -> ClientResult<SearchResult> {
        let mut params = HashMap::new();
        let limit = limit.into().unwrap_or(10);
        let offset = offset.into().unwrap_or(0);
        if let Some(_market) = market {
            params.insert("market".to_owned(), _market.as_str().to_owned());
        }
        if let Some(_include_external) = include_external {
            params.insert(
                "include_external".to_owned(),
                _include_external.as_str().to_owned(),
            );
        }
        params.insert("limit".to_owned(), limit.to_string());
        params.insert("offset".to_owned(), offset.to_string());
        params.insert("q".to_owned(), q.to_owned());
        params.insert("type".to_owned(), _type.as_str().to_owned());
        let url = String::from("search");
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<SearchResult>(&result)
    }

    /// [get albums tracks](https://developer.spotify.com/web-api/get-albums-tracks/)
    /// Get Spotify catalog information about an album's tracks
    /// Parameters:
    /// - album_id - the album ID, URI or URL
    /// - limit  - the number of items to return
    /// - offset - the index of the first item to return
    pub async fn album_track<L: Into<Option<u32>>, O: Into<Option<u32>>>(
        &self,
        album_id: &str,
        limit: L,
        offset: O,
    ) -> ClientResult<Page<SimplifiedTrack>> {
        let mut params = HashMap::new();
        let trid = self.get_id(Type::Album, album_id);
        let url = format!("albums/{}/tracks", trid);
        params.insert("limit".to_owned(), limit.into().unwrap_or(50).to_string());
        params.insert("offset".to_owned(), offset.into().unwrap_or(0).to_string());
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<Page<SimplifiedTrack>>(&result)
    }

    ///[get users profile](https://developer.spotify.com/web-api/get-users-profile/)
    ///Gets basic profile information about a Spotify User
    ///Parameters:
    ///- user - the id of the usr
    pub async fn user(&self, user_id: &str) -> ClientResult<PublicUser> {
        let url = format!("users/{}", user_id);
        let result = self.get(&url, &mut HashMap::new()).await?;
        self.convert_result::<PublicUser>(&result)
    }

    /// [get playlist](https://developer.spotify.com/documentation/web-api/reference/playlists/get-playlist/)
    /// Get full details about Spotify playlist
    /// Parameters:
    /// - playlist_id - the id of the playlist
    /// - market - an ISO 3166-1 alpha-2 country code.
    pub async fn playlist(
        &self,
        playlist_id: &str,
        fields: Option<&str>,
        market: Option<Country>,
    ) -> ClientResult<FullPlaylist> {
        let mut params = HashMap::new();
        if let Some(_fields) = fields {
            params.insert("fields".to_owned(), _fields.to_string());
        }
        if let Some(_market) = market {
            params.insert("market".to_owned(), _market.as_str().to_owned());
        }

        let plid = self.get_id(Type::Playlist, playlist_id);
        let url = format!("playlists/{}", plid);
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<FullPlaylist>(&result)
    }

    /// [get users playlists](https://developer.spotify.com/web-api/get-a-list-of-current-users-playlists/)
    /// Get current user playlists without required getting his profile
    /// Parameters:
    /// - limit  - the number of items to return
    /// - offset - the index of the first item to return
    pub async fn current_user_playlists<L: Into<Option<u32>>, O: Into<Option<u32>>>(
        &self,
        limit: L,
        offset: O,
    ) -> ClientResult<Page<SimplifiedPlaylist>> {
        let mut params = HashMap::new();
        params.insert("limit".to_owned(), limit.into().unwrap_or(50).to_string());
        params.insert("offset".to_owned(), offset.into().unwrap_or(0).to_string());

        let url = String::from("me/playlists");
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<Page<SimplifiedPlaylist>>(&result)
    }

    /// [get list users playlists](https://developer.spotify.com/web-api/get-list-users-playlists/)
    /// Gets playlists of a user
    /// Parameters:
    /// - user_id - the id of the usr
    /// - limit  - the number of items to return
    /// - offset - the index of the first item to return
    pub async fn user_playlists<L: Into<Option<u32>>, O: Into<Option<u32>>>(
        &self,
        user_id: &str,
        limit: L,
        offset: O,
    ) -> ClientResult<Page<SimplifiedPlaylist>> {
        let mut params = HashMap::new();
        params.insert("limit".to_owned(), limit.into().unwrap_or(50).to_string());
        params.insert("offset".to_owned(), offset.into().unwrap_or(0).to_string());
        let url = format!("users/{}/playlists", user_id);
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<Page<SimplifiedPlaylist>>(&result)
    }

    /// [get list users playlists](https://developer.spotify.com/web-api/get-list-users-playlists/)
    /// Gets playlist of a user
    /// Parameters:
    /// - user_id - the id of the user
    /// - playlist_id - the id of the playlist
    /// - fields - which fields to return
    pub async fn user_playlist(
        &self,
        user_id: &str,
        playlist_id: Option<&mut str>,
        fields: Option<&str>,
        market: Option<Country>,
    ) -> ClientResult<FullPlaylist> {
        let mut params = HashMap::new();
        if let Some(_fields) = fields {
            params.insert("fields".to_owned(), _fields.to_string());
        }
        if let Some(_market) = market {
            params.insert("market".to_owned(), _market.as_str().to_owned());
        }
        match playlist_id {
            Some(_playlist_id) => {
                let plid = self.get_id(Type::Playlist, _playlist_id);
                let url = format!("users/{}/playlists/{}", user_id, plid);
                let result = self.get(&url, &mut params).await?;
                self.convert_result::<FullPlaylist>(&result)
            }
            None => {
                let url = format!("users/{}/starred", user_id);
                let result = self.get(&url, &mut params).await?;
                self.convert_result::<FullPlaylist>(&result)
            }
        }
    }

    /// [get playlists tracks](https://developer.spotify.com/web-api/get-playlists-tracks/)
    /// Get full details of the tracks of a playlist owned by a user
    /// Parameters:
    /// - user_id - the id of the user
    /// - playlist_id - the id of the playlist
    /// - fields - which fields to return
    /// - limit - the maximum number of tracks to return
    /// - offset - the index of the first track to return
    /// - market - an ISO 3166-1 alpha-2 country code.
    pub async fn user_playlist_tracks<L: Into<Option<u32>>, O: Into<Option<u32>>>(
        &self,
        user_id: &str,
        playlist_id: &str,
        fields: Option<&str>,
        limit: L,
        offset: O,
        market: Option<Country>,
    ) -> ClientResult<Page<PlaylistTrack>> {
        let mut params = HashMap::new();
        params.insert("limit".to_owned(), limit.into().unwrap_or(50).to_string());
        params.insert("offset".to_owned(), offset.into().unwrap_or(0).to_string());
        if let Some(_market) = market {
            params.insert("market".to_owned(), _market.as_str().to_owned());
        }
        if let Some(_fields) = fields {
            params.insert("fields".to_owned(), _fields.to_string());
        }
        let plid = self.get_id(Type::Playlist, playlist_id);
        let url = format!("users/{}/playlists/{}/tracks", user_id, plid);
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<Page<PlaylistTrack>>(&result)
    }

    /// [create playlist](https://developer.spotify.com/web-api/create-playlist/)
    /// Creates a playlist for a user
    /// Parameters:
    /// - user_id - the id of the user
    /// - name - the name of the playlist
    /// - public - is the created playlist public
    /// - description - the description of the playlist
    pub async fn user_playlist_create<P: Into<Option<bool>>, D: Into<Option<String>>>(
        &self,
        user_id: &str,
        name: &str,
        public: P,
        description: D,
    ) -> ClientResult<FullPlaylist> {
        let public = public.into().unwrap_or(true);
        let description = description.into().unwrap_or_else(|| "".to_owned());
        let params = json!({
            "name": name,
            "public": public,
            "description": description
        });
        let url = format!("users/{}/playlists", user_id);
        let result = self.post(&url, &params).await?;
        self.convert_result::<FullPlaylist>(&result)
    }

    /// [change playlists details](https://developer.spotify.com/web-api/change-playlist-details/)
    /// Changes a playlist's name and/or public/private state
    /// Parameters:
    /// - user_id - the id of the user
    /// - playlist_id - the id of the playlist
    /// - name - optional name of the playlist
    /// - public - optional is the playlist public
    /// - collaborative - optional is the playlist collaborative
    /// - description - optional description of the playlist
    pub async fn user_playlist_change_detail(
        &self,
        user_id: &str,
        playlist_id: &str,
        name: Option<&str>,
        public: Option<bool>,
        description: Option<String>,
        collaborative: Option<bool>,
    ) -> ClientResult<String> {
        let mut params = Map::new();
        if let Some(_name) = name {
            params.insert("name".to_owned(), _name.into());
        }
        if let Some(_public) = public {
            params.insert("public".to_owned(), _public.into());
        }
        if let Some(_collaborative) = collaborative {
            params.insert("collaborative".to_owned(), _collaborative.into());
        }
        if let Some(_description) = description {
            params.insert("description".to_owned(), _description.into());
        }
        let url = format!("users/{}/playlists/{}", user_id, playlist_id);
        self.put(&url, &Value::Object(params)).await
    }

    /// [unfollow playlist](https://developer.spotify.com/web-api/unfollow-playlist/)
    /// Unfollows (deletes) a playlist for a user
    /// Parameters:
    /// - user_id - the id of the user
    /// - playlist_id - the id of the playlist
    pub async fn user_playlist_unfollow(
        &self,
        user_id: &str,
        playlist_id: &str,
    ) -> ClientResult<String> {
        let url = format!("users/{}/playlists/{}/followers", user_id, playlist_id);
        self.delete(&url, &json!({})).await
    }

    /// [add tracks to playlist](https://developer.spotify.com/web-api/add-tracks-to-playlist/)
    /// Adds tracks to a playlist
    /// Parameters:
    /// - user_id - the id of the user
    /// - playlist_id - the id of the playlist
    /// - track_ids - a list of track URIs, URLs or IDs
    /// - position - the position to add the tracks
    pub async fn user_playlist_add_tracks(
        &self,
        user_id: &str,
        playlist_id: &str,
        track_ids: impl IntoIterator<Item = &String>,
        position: Option<i32>,
    ) -> ClientResult<CUDResult> {
        let plid = self.get_id(Type::Playlist, playlist_id);
        let uris: Vec<String> = track_ids
            .into_iter()
            .map(|id| self.get_uri(Type::Track, id))
            .collect();
        let mut params = Map::new();
        if let Some(_position) = position {
            params.insert("position".to_owned(), _position.into());
        }
        params.insert("uris".to_owned(), uris.into());
        let url = format!("users/{}/playlists/{}/tracks", user_id, plid);
        let result = self.post(&url, &Value::Object(params)).await?;
        self.convert_result::<CUDResult>(&result)
    }
    ///[replaced playlists tracks](https://developer.spotify.com/web-api/replace-playlists-tracks/)
    ///Replace all tracks in a playlist
    ///Parameters:
    ///- user - the id of the user
    ///- playlist_id - the id of the playlist
    ///- tracks - the list of track ids to add to the playlist
    pub async fn user_playlist_replace_tracks(
        &self,
        user_id: &str,
        playlist_id: &str,
        track_ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<()> {
        let plid = self.get_id(Type::Playlist, playlist_id);
        let uris: Vec<String> = track_ids
            .into_iter()
            .map(|id| self.get_uri(Type::Track, id))
            .collect();
        // let mut params = Map::new();
        // params.insert("uris".to_owned(), uris.into());
        let params = json!({ "uris": uris });
        let url = format!("users/{}/playlists/{}/tracks", user_id, plid);
        match self.put(&url, &params).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [reorder playlists tracks](https://developer.spotify.com/web-api/reorder-playlists-tracks/)
    /// Reorder tracks in a playlist
    /// Parameters:
    /// - user_id - the id of the user
    /// - playlist_id - the id of the playlist
    /// - range_start - the position of the first track to be reordered
    /// - range_length - optional the number of tracks to be reordered (default: 1)
    /// - insert_before - the position where the tracks should be inserted
    /// - snapshot_id - optional playlist's snapshot ID
    pub async fn user_playlist_recorder_tracks<R: Into<Option<u32>>>(
        &self,
        user_id: &str,
        playlist_id: &str,
        range_start: i32,
        range_length: R,
        insert_before: i32,
        snapshot_id: Option<String>,
    ) -> ClientResult<CUDResult> {
        let plid = self.get_id(Type::Playlist, playlist_id);
        let range_length = range_length.into().unwrap_or(1);
        let mut params = Map::new();
        if let Some(_snapshot_id) = snapshot_id {
            params.insert("snapshot_id".to_owned(), _snapshot_id.into());
        }
        params.insert("range_start".to_owned(), range_start.into());
        params.insert("range_length".to_owned(), range_length.into());
        params.insert("insert_before".to_owned(), insert_before.into());
        let url = format!("users/{}/playlists/{}/tracks", user_id, plid);
        let result = self.put(&url, &Value::Object(params)).await?;
        self.convert_result::<CUDResult>(&result)
    }

    /// [remove tracks playlist](https://developer.spotify.com/web-api/remove-tracks-playlist/)
    /// Removes all occurrences of the given tracks from the given playlist
    /// Parameters:
    /// - user_id - the id of the user
    /// - playlist_id - the id of the playlist
    /// - track_ids - the list of track ids to add to the playlist
    /// - snapshot_id - optional id of the playlist snapshot
    pub async fn user_playlist_remove_all_occurrences_of_tracks(
        &self,
        user_id: &str,
        playlist_id: &str,
        track_ids: impl IntoIterator<Item = &String>,
        snapshot_id: Option<String>,
    ) -> ClientResult<CUDResult> {
        let plid = self.get_id(Type::Playlist, playlist_id);
        let uris: Vec<String> = track_ids
            .into_iter()
            .map(|id| self.get_uri(Type::Track, id))
            .collect();
        let mut params = Map::new();
        let mut tracks: Vec<Map<String, Value>> = vec![];
        for uri in uris {
            let mut map = Map::new();
            map.insert("uri".to_owned(), uri.into());
            tracks.push(map);
        }
        params.insert("tracks".to_owned(), tracks.into());
        if let Some(_snapshot_id) = snapshot_id {
            params.insert("snapshot_id".to_owned(), _snapshot_id.into());
        }
        let url = format!("users/{}/playlists/{}/tracks", user_id, plid);
        let result = self.delete(&url, &Value::Object(params)).await?;
        self.convert_result::<CUDResult>(&result)
    }

    /// [remove tracks playlist](https://developer.spotify.com/web-api/remove-tracks-playlist/)
    /// Removes specfic occurrences of the given tracks from the given playlist
    /// Parameters:
    /// - user_id: the id of the user
    /// - playlist_id: the id of the playlist
    /// - tracks: an array of map containing Spotify URIs of the tracks to remove
    /// with their current positions in the playlist. For example:
    ///
    /// ```json
    /// {
    ///    "tracks":[
    ///       {
    ///          "uri":"spotify:track:4iV5W9uYEdYUVa79Axb7Rh",
    ///          "positions":[
    ///             0,
    ///             3
    ///          ]
    ///       },
    ///       {
    ///          "uri":"spotify:track:1301WleyT98MSxVHPZCA6M",
    ///          "positions":[
    ///             7
    ///          ]
    ///       }
    ///    ]
    /// }
    /// ```
    /// - snapshot_id: optional id of the playlist snapshot
    pub async fn user_playlist_remove_specific_occurrences_of_tracks(
        &self,
        user_id: &str,
        playlist_id: &str,
        tracks: Vec<Map<String, Value>>,
        snapshot_id: Option<String>,
    ) -> ClientResult<CUDResult> {
        let mut params = Map::new();
        let plid = self.get_id(Type::Playlist, playlist_id);
        let mut ftracks: Vec<Map<String, Value>> = vec![];
        for track in tracks {
            let mut map = Map::new();
            if let Some(_uri) = track.get("uri") {
                let uri = self.get_uri(Type::Track, &_uri.as_str().unwrap().to_owned());
                map.insert("uri".to_owned(), uri.into());
            }
            if let Some(_position) = track.get("position") {
                map.insert("position".to_owned(), _position.to_owned());
            }
            ftracks.push(map);
        }
        params.insert("tracks".to_owned(), ftracks.into());
        if let Some(_snapshot_id) = snapshot_id {
            params.insert("snapshot_id".to_owned(), _snapshot_id.into());
        }
        let url = format!("users/{}/playlists/{}/tracks", user_id, plid);
        let result = self.delete(&url, &Value::Object(params)).await?;
        self.convert_result::<CUDResult>(&result)
    }

    /// [follow playlist](https://developer.spotify.com/web-api/follow-playlist/)
    /// Add the current authenticated user as a follower of a playlist.
    /// Parameters:
    /// - playlist_owner_id - the user id of the playlist owner
    /// - playlist_id - the id of the playlist
    pub async fn user_playlist_follow_playlist<P: Into<Option<bool>>>(
        &self,
        playlist_owner_id: &str,
        playlist_id: &str,
        public: P,
    ) -> ClientResult<()> {
        let mut map = Map::new();
        let public = public.into().unwrap_or(true);
        map.insert("public".to_owned(), public.into());
        let url = format!(
            "users/{}/playlists/{}/followers",
            playlist_owner_id, playlist_id
        );
        match self.put(&url, &Value::Object(map)).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [check user following playlist](https://developer.spotify.com/web-api/check-user-following-playlist/)
    /// Check to see if the given users are following the given playlist
    /// Parameters:
    /// - playlist_owner_id - the user id of the playlist owner
    /// - playlist_id - the id of the playlist
    /// - user_ids - the ids of the users that you want to
    /// check to see if they follow the playlist. Maximum: 5 ids.
    pub async fn user_playlist_check_follow(
        &self,
        playlist_owner_id: &str,
        playlist_id: &str,
        user_ids: &[String],
    ) -> ClientResult<Vec<bool>> {
        if user_ids.len() > 5 {
            error!("The maximum length of user ids is limited to 5 :-)");
        }
        let url = format!(
            "users/{}/playlists/{}/followers/contains?ids={}",
            playlist_owner_id,
            playlist_id,
            user_ids.join(",")
        );
        let mut dumb: HashMap<String, String> = HashMap::new();
        let result = self.get(&url, &mut dumb).await?;
        self.convert_result::<Vec<bool>>(&result)
    }
    /// [get current users profile](https://developer.spotify.com/web-api/get-current-users-profile/)
    /// Get detailed profile information about the current user.
    /// An alias for the 'current_user' method.
    pub async fn me(&self) -> ClientResult<PrivateUser> {
        let mut dumb: HashMap<String, String> = HashMap::new();
        let url = String::from("me/");
        let result = self.get(&url, &mut dumb).await?;
        self.convert_result::<PrivateUser>(&result)
    }
    /// Get detailed profile information about the current user.
    /// An alias for the 'me' method.
    pub async fn current_user(&self) -> ClientResult<PrivateUser> {
        self.me().await
    }

    ///  [get the users currently playing track](https://developer.spotify.com/web-api/get-the-users-currently-playing-track/)
    ///  Get information about the current users currently playing track.
    pub async fn current_user_playing_track(&self) -> ClientResult<Option<Playing>> {
        let mut dumb = HashMap::new();
        let url = String::from("me/player/currently-playing");
        match self.get(&url, &mut dumb).await {
            Ok(result) => {
                if result.is_empty() {
                    Ok(None)
                } else {
                    self.convert_result::<Option<Playing>>(&result)
                }
            }
            Err(e) => Err(e),
        }
    }

    /// [get user saved albums](https://developer.spotify.com/web-api/get-users-saved-albums/)
    /// Gets a list of the albums saved in the current authorized user's
    /// "Your Music" library
    /// Parameters:
    /// - limit - the number of albums to return
    /// - offset - the index of the first album to return
    /// - market - Provide this parameter if you want to apply Track Relinking.
    pub async fn current_user_saved_albums<L: Into<Option<u32>>, O: Into<Option<u32>>>(
        &self,
        limit: L,
        offset: O,
    ) -> ClientResult<Page<SavedAlbum>> {
        let limit = limit.into().unwrap_or(20);
        let offset = offset.into().unwrap_or(0);
        let mut params = HashMap::new();
        params.insert("limit".to_owned(), limit.to_string());
        params.insert("offset".to_owned(), offset.to_string());
        let url = String::from("me/albums");
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<Page<SavedAlbum>>(&result)
    }
    ///[get users saved tracks](https://developer.spotify.com/web-api/get-users-saved-tracks/)
    ///Parameters:
    ///- limit - the number of tracks to return
    ///- offset - the index of the first track to return
    ///- market - Provide this parameter if you want to apply Track Relinking.
    pub async fn current_user_saved_tracks<L: Into<Option<u32>>, O: Into<Option<u32>>>(
        &self,
        limit: L,
        offset: O,
    ) -> ClientResult<Page<SavedTrack>> {
        let limit = limit.into().unwrap_or(20);
        let offset = offset.into().unwrap_or(0);
        let mut params = HashMap::new();
        params.insert("limit".to_owned(), limit.to_string());
        params.insert("offset".to_owned(), offset.to_string());
        let url = String::from("me/tracks");
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<Page<SavedTrack>>(&result)
    }
    ///[get followed artists](https://developer.spotify.com/web-api/get-followed-artists/)
    ///Gets a list of the artists followed by the current authorized user
    ///Parameters:
    ///- limit - the number of tracks to return
    ///- after - ghe last artist ID retrieved from the previous request
    pub async fn current_user_followed_artists<L: Into<Option<u32>>>(
        &self,
        limit: L,
        after: Option<String>,
    ) -> ClientResult<CursorPageFullArtists> {
        let limit = limit.into().unwrap_or(20);
        let mut params = HashMap::new();
        params.insert("limit".to_owned(), limit.to_string());
        if let Some(_after) = after {
            params.insert("after".to_owned(), _after);
        }
        params.insert("type".to_owned(), Type::Artist.as_str().to_owned());
        let url = String::from("me/following");
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<CursorPageFullArtists>(&result)
    }

    /// [remove tracks users](https://developer.spotify.com/web-api/remove-tracks-user/)
    /// Remove one or more tracks from the current user's
    /// "Your Music" library.
    /// Parameters:
    /// - track_ids - a list of track URIs, URLs or IDs
    pub async fn current_user_saved_tracks_delete(
        &self,
        track_ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<()> {
        let uris: Vec<String> = track_ids
            .into_iter()
            .map(|id| self.get_id(Type::Track, id))
            .collect();
        let url = format!("me/tracks/?ids={}", uris.join(","));
        match self.delete(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [check users saved tracks](https://developer.spotify.com/web-api/check-users-saved-tracks/)
    /// Check if one or more tracks is already saved in
    /// the current Spotify user’s “Your Music” library.
    /// Parameters:
    /// - track_ids - a list of track URIs, URLs or IDs
    pub async fn current_user_saved_tracks_contains(
        &self,
        track_ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<Vec<bool>> {
        let uris: Vec<String> = track_ids
            .into_iter()
            .map(|id| self.get_id(Type::Track, id))
            .collect();
        let url = format!("me/tracks/contains/?ids={}", uris.join(","));
        let mut dumb = HashMap::new();
        let result = self.get(&url, &mut dumb).await?;
        self.convert_result::<Vec<bool>>(&result)
    }

    /// [save tracks user ](https://developer.spotify.com/web-api/save-tracks-user/)
    /// Save one or more tracks to the current user's
    /// "Your Music" library.
    /// Parameters:
    /// - track_ids - a list of track URIs, URLs or IDs
    pub async fn current_user_saved_tracks_add(
        &self,
        track_ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<()> {
        let uris: Vec<String> = track_ids
            .into_iter()
            .map(|id| self.get_id(Type::Track, id))
            .collect();
        let url = format!("me/tracks/?ids={}", uris.join(","));
        match self.put(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [get users  top artists and tracks](https://developer.spotify.com/web-api/get-users-top-artists-and-tracks/)
    /// Get the current user's top artists
    /// Parameters:
    /// - limit - the number of entities to return
    /// - offset - the index of the first entity to return
    /// - time_range - Over what time frame are the affinities computed
    pub async fn current_user_top_artists<
        L: Into<Option<u32>>,
        O: Into<Option<u32>>,
        T: Into<Option<TimeRange>>,
    >(
        &self,
        limit: L,
        offset: O,
        time_range: T,
    ) -> ClientResult<Page<FullArtist>> {
        let limit = limit.into().unwrap_or(20);
        let offset = offset.into().unwrap_or(0);
        let time_range = time_range.into().unwrap_or(TimeRange::MediumTerm);
        let mut params = HashMap::new();
        params.insert("limit".to_owned(), limit.to_string());
        params.insert("offset".to_owned(), offset.to_string());
        params.insert("time_range".to_owned(), time_range.as_str().to_owned());
        let url = String::from("me/top/artists");
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<Page<FullArtist>>(&result)
    }

    /// [get users top artists and tracks](https://developer.spotify.com/web-api/get-users-top-artists-and-tracks/)
    /// Get the current user's top tracks
    /// Parameters:
    /// - limit - the number of entities to return
    /// - offset - the index of the first entity to return
    /// - time_range - Over what time frame are the affinities computed
    pub async fn current_user_top_tracks<
        L: Into<Option<u32>>,
        O: Into<Option<u32>>,
        T: Into<Option<TimeRange>>,
    >(
        &self,
        limit: L,
        offset: O,
        time_range: T,
    ) -> ClientResult<Page<FullTrack>> {
        let limit = limit.into().unwrap_or(20);
        let offset = offset.into().unwrap_or(0);
        let time_range = time_range.into().unwrap_or(TimeRange::MediumTerm);
        let mut params = HashMap::new();
        params.insert("limit".to_owned(), limit.to_string());
        params.insert("offset".to_owned(), offset.to_string());
        params.insert("time_range".to_owned(), time_range.as_str().to_owned());
        let url = String::from("me/top/tracks");
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<Page<FullTrack>>(&result)
    }

    /// [get recently played](https://developer.spotify.com/web-api/web-api-personalization-endpoints/get-recently-played/)
    /// Get the current user's recently played tracks
    /// Parameters:
    /// - limit - the number of entities to return
    pub async fn current_user_recently_played<L: Into<Option<u32>>>(
        &self,
        limit: L,
    ) -> ClientResult<CursorBasedPage<PlayHistory>> {
        let limit = limit.into().unwrap_or(50);
        let mut params = HashMap::new();
        params.insert("limit".to_owned(), limit.to_string());
        let url = String::from("me/player/recently-played");
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<CursorBasedPage<PlayHistory>>(&result)
    }

    /// [save albums user](https://developer.spotify.com/web-api/save-albums-user/)
    /// Add one or more albums to the current user's
    /// "Your Music" library.
    /// Parameters:
    /// - album_ids - a list of album URIs, URLs or IDs
    pub async fn current_user_saved_albums_add(
        &self,
        album_ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<()> {
        let uris: Vec<String> = album_ids
            .into_iter()
            .map(|id| self.get_id(Type::Album, id))
            .collect();
        let url = format!("me/albums/?ids={}", uris.join(","));
        match self.put(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [remove albums user](https://developer.spotify.com/documentation/web-api/reference/library/remove-albums-user/)
    /// Remove one or more albums from the current user's
    /// "Your Music" library.
    /// Parameters:
    /// - album_ids - a list of album URIs, URLs or IDs
    pub async fn current_user_saved_albums_delete(
        &self,
        album_ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<()> {
        let uris: Vec<String> = album_ids
            .into_iter()
            .map(|id| self.get_id(Type::Album, id))
            .collect();
        let url = format!("me/albums/?ids={}", uris.join(","));
        match self.delete(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [check users saved albums](https://developer.spotify.com/documentation/web-api/reference/library/check-users-saved-albums/)
    /// Check if one or more albums is already saved in
    /// the current Spotify user’s “Your Music” library.
    /// Parameters:
    /// - album_ids - a list of album URIs, URLs or IDs
    pub async fn current_user_saved_albums_contains(
        &self,
        album_ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<Vec<bool>> {
        let uris: Vec<String> = album_ids
            .into_iter()
            .map(|id| self.get_id(Type::Album, id))
            .collect();
        let url = format!("me/albums/contains/?ids={}", uris.join(","));
        let mut dumb = HashMap::new();
        let result = self.get(&url, &mut dumb).await?;
        self.convert_result::<Vec<bool>>(&result)
    }

    /// [follow artists users](https://developer.spotify.com/web-api/follow-artists-users/)
    /// Follow one or more artists
    /// Parameters:
    /// - artist_ids - a list of artist IDs
    pub async fn user_follow_artists(
        &self,
        artist_ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<()> {
        let url = format!(
            "me/following?type=artist&ids={}",
            artist_ids
                .into_iter()
                .map(|s| s.as_ref())
                .collect::<Vec<&str>>()
                .join(",")
        );
        match self.put(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [unfollow artists users](https://developer.spotify.com/documentation/web-api/reference/follow/unfollow-artists-users/)
    /// Unfollow one or more artists
    /// Parameters:
    /// - artist_ids - a list of artist IDs
    pub async fn user_unfollow_artists(
        &self,
        artist_ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<()> {
        let url = format!(
            "me/following?type=artist&ids={}",
            artist_ids
                .into_iter()
                .map(|s| s.as_ref())
                .collect::<Vec<&str>>()
                .join(",")
        );
        match self.delete(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [check user following
    /// artists](https://developer.spotify.com/web-api/checkcurrent-user-follows/)
    /// Check to see if the given users are following the given artists
    /// Parameters:
    /// - artist_ids - the ids of the users that you want to
    pub async fn user_artist_check_follow(
        &self,
        artsit_ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<Vec<bool>> {
        let url = format!(
            "me/following/contains?type=artist&ids={}",
            artsit_ids
                .into_iter()
                .map(|s| s.as_ref())
                .collect::<Vec<&str>>()
                .join(",")
        );
        let mut dumb = HashMap::new();
        let result = self.get(&url, &mut dumb).await?;
        self.convert_result::<Vec<bool>>(&result)
    }

    /// [follow artists users](https://developer.spotify.com/web-api/follow-artists-users/)
    /// Follow one or more users
    /// Parameters:
    /// - user_ids - a list of artist IDs
    pub async fn user_follow_users(
        &self,
        user_ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<()> {
        let url = format!(
            "me/following?type=user&ids={}",
            user_ids
                .into_iter()
                .map(|s| s.as_ref())
                .collect::<Vec<&str>>()
                .join(",")
        );
        match self.put(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [unfollow artists users](https://developer.spotify.com/documentation/web-api/reference/follow/unfollow-artists-users/)
    /// Unfollow one or more users
    /// Parameters:
    /// - user_ids - a list of artist IDs
    pub async fn user_unfollow_users(
        &self,
        user_ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<()> {
        let url = format!(
            "me/following?type=user&ids={}",
            user_ids
                .into_iter()
                .map(|s| s.as_ref())
                .collect::<Vec<&str>>()
                .join(",")
        );
        match self.delete(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [get list featured playlists](https://developer.spotify.com/web-api/get-list-featured-playlists/)
    /// Get a list of Spotify featured playlists
    /// Parameters:
    /// - locale - The desired language, consisting of a lowercase ISO
    /// 639 language code and an uppercase ISO 3166-1 alpha-2 country
    /// code, joined by an underscore.
    /// - country - An ISO 3166-1 alpha-2 country code.
    /// - timestamp - A timestamp in ISO 8601 format:
    /// yyyy-MM-ddTHH:mm:ss. Use this parameter to specify the user's
    /// local time to get results tailored for that specific date and
    /// time in the day
    /// - limit - The maximum number of items to return. Default: 20.
    /// Minimum: 1. Maximum: 50
    /// - offset - The index of the first item to return. Default: 0
    /// (the first object). Use with limit to get the next set of
    /// items.
    pub async fn featured_playlists<L: Into<Option<u32>>, O: Into<Option<u32>>>(
        &self,
        locale: Option<String>,
        country: Option<Country>,
        timestamp: Option<DateTime<Utc>>,
        limit: L,
        offset: O,
    ) -> ClientResult<FeaturedPlaylists> {
        let mut params = HashMap::new();
        let limit = limit.into().unwrap_or(20);
        let offset = offset.into().unwrap_or(0);
        if let Some(_locale) = locale {
            params.insert("locale".to_owned(), _locale);
        }
        if let Some(_country) = country {
            params.insert("country".to_owned(), _country.as_str().to_owned());
        }
        if let Some(_timestamp) = timestamp {
            params.insert("timestamp".to_owned(), _timestamp.to_rfc3339());
        }
        params.insert("limit".to_owned(), limit.to_string());
        params.insert("offset".to_owned(), offset.to_string());
        let url = String::from("browse/featured-playlists");
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<FeaturedPlaylists>(&result)
    }

    /// [get list new releases](https://developer.spotify.com/web-api/get-list-new-releases/)
    /// Get a list of new album releases featured in Spotify
    /// Parameters:
    /// - country - An ISO 3166-1 alpha-2 country code.
    /// - limit - The maximum number of items to return. Default: 20.
    /// Minimum: 1. Maximum: 50
    /// - offset - The index of the first item to return. Default: 0
    /// (the first object). Use with limit to get the next set of
    /// items.
    pub async fn new_releases<L: Into<Option<u32>>, O: Into<Option<u32>>>(
        &self,
        country: Option<Country>,
        limit: L,
        offset: O,
    ) -> ClientResult<PageSimpliedAlbums> {
        let mut params = HashMap::new();
        let limit = limit.into().unwrap_or(20);
        let offset = offset.into().unwrap_or(0);
        if let Some(_country) = country {
            params.insert("country".to_owned(), _country.as_str().to_owned());
        }
        params.insert("limit".to_owned(), limit.to_string());
        params.insert("offset".to_owned(), offset.to_string());
        let url = String::from("browse/new-releases");
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<PageSimpliedAlbums>(&result)
    }

    /// [get list categories](https://developer.spotify.com/web-api/get-list-categories/)
    /// Get a list of new album releases featured in Spotify
    /// Parameters:
    /// - country - An ISO 3166-1 alpha-2 country code.
    /// - locale - The desired language, consisting of an ISO 639
    /// language code and an ISO 3166-1 alpha-2 country code, joined
    /// by an underscore.
    /// - limit - The maximum number of items to return. Default: 20.
    /// Minimum: 1. Maximum: 50
    /// - offset - The index of the first item to return. Default: 0
    /// (the first object). Use with limit to get the next set of
    /// items.
    pub async fn categories<L: Into<Option<u32>>, O: Into<Option<u32>>>(
        &self,
        locale: Option<String>,
        country: Option<Country>,
        limit: L,
        offset: O,
    ) -> ClientResult<PageCategory> {
        let mut params = HashMap::new();
        let limit = limit.into().unwrap_or(20);
        let offset = offset.into().unwrap_or(0);
        if let Some(_locale) = locale {
            params.insert("locale".to_owned(), _locale);
        }
        if let Some(_country) = country {
            params.insert("country".to_owned(), _country.as_str().to_owned());
        }
        params.insert("limit".to_owned(), limit.to_string());
        params.insert("offset".to_owned(), offset.to_string());
        let url = String::from("browse/categories");
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<PageCategory>(&result)
    }

    /// [get recommendtions](https://developer.spotify.com/web-api/get-recommendations/)
    /// Get Recommendations Based on Seeds
    /// Parameters:
    /// - seed_artists - a list of artist IDs, URIs or URLs
    /// - seed_tracks - a list of artist IDs, URIs or URLs
    /// - seed_genres - a list of genre names. Available genres for
    /// - country - An ISO 3166-1 alpha-2 country code. If provided, all
    ///   results will be playable in this country.
    /// - limit - The maximum number of items to return. Default: 20.
    ///   Minimum: 1. Maximum: 100
    /// - min/max/target_<attribute> - For the tuneable track attributes listed
    ///   in the documentation, these values provide filters and targeting on
    ///   results.
    pub async fn recommendations<L: Into<Option<u32>>>(
        &self,
        seed_artists: Option<Vec<String>>,
        seed_genres: Option<Vec<String>>,
        seed_tracks: Option<Vec<String>>,
        limit: L,
        country: Option<Country>,
        payload: &Map<String, Value>,
    ) -> ClientResult<Recommendations> {
        let mut params = HashMap::new();
        let limit = limit.into().unwrap_or(20);
        params.insert("limit".to_owned(), limit.to_string());
        if let Some(_seed_artists) = seed_artists {
            let seed_artists_ids: Vec<String> = _seed_artists
                .iter()
                .map(|id| self.get_id(Type::Artist, id))
                .collect();
            params.insert("seed_artists".to_owned(), seed_artists_ids.join(","));
        }
        if let Some(_seed_genres) = seed_genres {
            params.insert("seed_genres".to_owned(), _seed_genres.join(","));
        }
        if let Some(_seed_tracks) = seed_tracks {
            let seed_tracks_ids: Vec<String> = _seed_tracks
                .iter()
                .map(|id| self.get_id(Type::Track, id))
                .collect();
            params.insert("seed_tracks".to_owned(), seed_tracks_ids.join(","));
        }
        if let Some(_country) = country {
            params.insert("market".to_owned(), _country.as_str().to_owned());
        }
        let attributes = [
            "acousticness",
            "danceability",
            "duration_ms",
            "energy",
            "instrumentalness",
            "key",
            "liveness",
            "loudness",
            "mode",
            "popularity",
            "speechiness",
            "tempo",
            "time_signature",
            "valence",
        ];
        let prefixes = ["min", "max", "target"];
        for attribute in attributes.iter() {
            for prefix in prefixes.iter() {
                let param = format!("{}_{}", prefix, attribute);
                if let Some(value) = payload.get(&param) {
                    params.insert(param, value.to_string());
                }
            }
        }
        let url = String::from("recommendations");
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<Recommendations>(&result)
    }
    /// [get audio features](https://developer.spotify.com/web-api/get-audio-features/)
    /// Get audio features for a track
    /// - track - track URI, URL or ID
    pub async fn audio_features(&self, track: &str) -> ClientResult<AudioFeatures> {
        let track_id = self.get_id(Type::Track, track);
        let url = format!("audio-features/{}", track_id);
        let mut dumb = HashMap::new();
        let result = self.get(&url, &mut dumb).await?;
        self.convert_result::<AudioFeatures>(&result)
    }

    /// [get several audio features](https://developer.spotify.com/web-api/get-several-audio-features/)
    /// Get Audio Features for Several Tracks
    /// - tracks a list of track URIs, URLs or IDs
    pub async fn audios_features(
        &self,
        tracks: impl IntoIterator<Item = &String>,
    ) -> ClientResult<Option<AudioFeaturesPayload>> {
        let ids: Vec<String> = tracks
            .into_iter()
            .map(|track| self.get_id(Type::Track, track))
            .collect();
        let url = format!("audio-features/?ids={}", ids.join(","));
        let mut dumb = HashMap::new();
        match self.get(&url, &mut dumb).await {
            Ok(result) => {
                if result.is_empty() {
                    Ok(None)
                } else {
                    self.convert_result::<Option<AudioFeaturesPayload>>(&result)
                }
            }
            Err(e) => Err(e),
        }
    }

    /// [get audio analysis](https://developer.spotify.com/web-api/get-audio-analysis/)
    /// Get Audio Analysis for a Track
    /// Parameters:
    /// - track_id - a track URI, URL or ID
    pub async fn audio_analysis(&self, track: &str) -> ClientResult<AudioAnalysis> {
        let trid = self.get_id(Type::Track, track);
        let url = format!("audio-analysis/{}", trid);
        let mut dumb = HashMap::new();
        let result = self.get(&url, &mut dumb).await?;
        self.convert_result::<AudioAnalysis>(&result)
    }

    /// [get a users available devices](https://developer.spotify.com/web-api/get-a-users-available-devices/)
    /// Get a User’s Available Devices
    pub async fn device(&self) -> ClientResult<DevicePayload> {
        let url = String::from("me/player/devices");
        let mut dumb = HashMap::new();
        let result = self.get(&url, &mut dumb).await?;
        self.convert_result::<DevicePayload>(&result)
    }

    /// [get informatation about the users  current playback](https://developer.spotify.com/web-api/get-information-about-the-users-current-playback/)
    /// Get Information About The User’s Current Playback
    /// Parameters:
    /// - market: Optional. an ISO 3166-1 alpha-2 country code.
    /// - additional_types: Optional. A comma-separated list of item types that your client supports besides the default track type. Valid types are: `track` and `episode`.
    pub async fn current_playback(
        &self,
        market: Option<Country>,
        additional_types: Option<Vec<AdditionalType>>,
    ) -> ClientResult<Option<CurrentlyPlaybackContext>> {
        let url = String::from("me/player");
        let mut params = HashMap::new();
        if let Some(_market) = market {
            params.insert("country".to_owned(), _market.as_str().to_owned());
        }
        if let Some(_additional_types) = additional_types {
            params.insert(
                "additional_types".to_owned(),
                _additional_types
                    .iter()
                    .map(|&x| x.as_str().to_owned())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        match self.get(&url, &mut params).await {
            Ok(result) => {
                if result.is_empty() {
                    Ok(None)
                } else {
                    self.convert_result::<Option<CurrentlyPlaybackContext>>(&result)
                }
            }
            Err(e) => Err(e),
        }
    }

    /// [get the users currently playing track](https://developer.spotify.com/web-api/get-the-users-currently-playing-track/)
    /// Get the User’s Currently Playing Track
    /// Parameters:
    /// - market: Optional. an ISO 3166-1 alpha-2 country code.
    /// - additional_types: Optional. A comma-separated list of item types that your client supports besides the default track type. Valid types are: `track` and `episode`.
    pub async fn current_playing(
        &self,
        market: Option<Country>,
        additional_types: Option<Vec<AdditionalType>>,
    ) -> ClientResult<Option<CurrentlyPlayingContext>> {
        let url = String::from("me/player/currently-playing");
        let mut params = HashMap::new();
        if let Some(_market) = market {
            params.insert("country".to_owned(), _market.as_str().to_owned());
        }
        if let Some(_additional_types) = additional_types {
            params.insert(
                "additional_types".to_owned(),
                _additional_types
                    .iter()
                    .map(|&x| x.as_str().to_owned())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        match self.get(&url, &mut params).await {
            Ok(result) => {
                if result.is_empty() {
                    Ok(None)
                } else {
                    self.convert_result::<Option<CurrentlyPlayingContext>>(&result)
                }
            }
            Err(e) => Err(e),
        }
    }
    /// [transfer a users playback](https://developer.spotify.com/web-api/transfer-a-users-playback/)
    /// Transfer a User’s Playback
    /// Note: Although an array is accepted, only a single device_id is currently
    /// supported. Supplying more than one will return 400 Bad Request
    /// Parameters:
    /// - device_id - transfer playback to this device
    /// - force_play - true: after transfer, play. false:
    /// keep current state.
    pub async fn transfer_playback<T: Into<Option<bool>>>(
        &self,
        device_id: &str,
        force_play: T,
    ) -> ClientResult<()> {
        let device_ids = vec![device_id.to_owned()];
        let force_play = force_play.into().unwrap_or(true);
        let mut payload = Map::new();
        payload.insert("device_ids".to_owned(), device_ids.into());
        payload.insert("play".to_owned(), force_play.into());
        let url = String::from("me/player");
        match self.put(&url, &Value::Object(payload)).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [start a users playback](https://developer.spotify.com/web-api/start-a-users-playback/)
    /// Start/Resume a User’s Playback
    /// Provide a `context_uri` to start playback or a album,
    /// artist, or playlist.
    ///
    /// Provide a `uris` list to start playback of one or more
    /// tracks.
    ///
    /// Provide `offset` as {"position": <int>} or {"uri": "<track uri>"}
    /// to start playback at a particular offset.
    ///
    /// Parameters:
    /// - device_id - device target for playback
    /// - context_uri - spotify context uri to play
    /// - uris - spotify track uris
    /// - offset - offset into context by index or track
    /// - position_ms - Indicates from what position to start playback.
    pub async fn start_playback(
        &self,
        device_id: Option<String>,
        context_uri: Option<String>,
        uris: Option<Vec<String>>,
        offset: Option<super::model::offset::Offset>,
        position_ms: Option<u32>,
    ) -> ClientResult<()> {
        if context_uri.is_some() && uris.is_some() {
            error!("specify either contexxt uri or uris, not both");
        }
        let mut params = Map::new();
        if let Some(_context_uri) = context_uri {
            params.insert("context_uri".to_owned(), _context_uri.into());
        }
        if let Some(_uris) = uris {
            params.insert("uris".to_owned(), _uris.into());
        }
        if let Some(_offset) = offset {
            if let Some(_position) = _offset.position {
                let mut offset_map = Map::new();
                offset_map.insert("position".to_owned(), _position.into());
                params.insert("offset".to_owned(), offset_map.into());
            } else if let Some(_uri) = _offset.uri {
                let mut offset_map = Map::new();
                offset_map.insert("uri".to_owned(), _uri.into());
                params.insert("offset".to_owned(), offset_map.into());
            }
        }
        if let Some(_position_ms) = position_ms {
            params.insert("position_ms".to_owned(), _position_ms.into());
        };
        let url = self.append_device_id("me/player/play", device_id);
        match self.put(&url, &Value::Object(params)).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [pause a users playback](https://developer.spotify.com/web-api/pause-a-users-playback/)
    /// Pause a User’s Playback
    /// Parameters:
    /// - device_id - device target for playback
    pub async fn pause_playback(&self, device_id: Option<String>) -> ClientResult<()> {
        let url = self.append_device_id("me/player/pause", device_id);
        match self.put(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [skip users playback to the next track](https://developer.spotify.com/web-api/skip-users-playback-to-next-track/)
    /// Skip User’s Playback To Next Track
    /// Parameters:
    /// - device_id - device target for playback
    pub async fn next_track(&self, device_id: Option<String>) -> ClientResult<()> {
        let url = self.append_device_id("me/player/next", device_id);
        match self.post(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [skip users playback to previous track](https://developer.spotify.com/web-api/skip-users-playback-to-previous-track/)
    /// Skip User’s Playback To Previous Track
    /// Parameters:
    /// - device_id - device target for playback
    pub async fn previous_track(&self, device_id: Option<String>) -> ClientResult<()> {
        let url = self.append_device_id("me/player/previous", device_id);
        match self.post(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [seek-to-position-in-currently-playing-track/](https://developer.spotify.com/web-api/seek-to-position-in-currently-playing-track/)
    /// Seek To Position In Currently Playing Track
    /// Parameters:
    /// - position_ms - position in milliseconds to seek to
    /// - device_id - device target for playback
    pub async fn seek_track(
        &self,
        position_ms: u32,
        device_id: Option<String>,
    ) -> ClientResult<()> {
        let url = self.append_device_id(
            &format!("me/player/seek?position_ms={}", position_ms),
            device_id,
        );
        match self.put(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [set repeat mode on users playback](https://developer.spotify.com/web-api/set-repeat-mode-on-users-playback/)
    /// Set Repeat Mode On User’s Playback
    /// Parameters:
    ///  - state - `track`, `context`, or `off`
    ///  - device_id - device target for playback
    pub async fn repeat(&self, state: RepeatState, device_id: Option<String>) -> ClientResult<()> {
        let url = self.append_device_id(
            &format!("me/player/repeat?state={}", state.as_str()),
            device_id,
        );
        match self.put(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [set-volume-for-users-playback](https://developer.spotify.com/web-api/set-volume-for-users-playback/)
    /// Set Volume For User’s Playback
    /// Parameters:
    /// - volume_percent - volume between 0 and 100
    /// - device_id - device target for playback
    pub async fn volume(&self, volume_percent: u8, device_id: Option<String>) -> ClientResult<()> {
        if volume_percent > 100u8 {
            error!("volume must be between 0 and 100, inclusive");
        }
        let url = self.append_device_id(
            &format!("me/player/volume?volume_percent={}", volume_percent),
            device_id,
        );
        match self.put(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [toggle shuffle for user playback](https://developer.spotify.com/web-api/toggle-shuffle-for-users-playback/)
    /// Toggle Shuffle For User’s Playback
    /// Parameters:
    /// - state - true or false
    /// - device_id - device target for playback
    pub async fn shuffle(&self, state: bool, device_id: Option<String>) -> ClientResult<()> {
        let url = self.append_device_id(&format!("me/player/shuffle?state={}", state), device_id);
        match self.put(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [Add an item to the end fo the user's current playback queue](https://developer.spotify.com/console/post-queue/)
    /// Add an item to the end of the user's playback queue
    /// Parameters:
    /// - uri - THe uri of the item to add, Track or Episode
    /// - device id - The id of the device targeting
    /// - If no device ID provided the user's currently active device is targeted
    pub async fn add_item_to_queue(
        &self,
        item: String,
        device_id: Option<String>,
    ) -> ClientResult<()> {
        let url = self.append_device_id(&format!("me/player/queue?uri={}", &item), device_id);
        match self.post(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// [Save Shows for Current User](https://developer.spotify.com/console/put-current-user-saved-shows)
    /// Add a show or a list of shows to a user’s library
    /// Parameters:
    /// - ids(Required) A comma-separated list of Spotify IDs for the shows to be added to the user’s library.
    pub async fn save_shows(&self, ids: impl IntoIterator<Item = &String>) -> ClientResult<()> {
        let joined_ids = ids
            .into_iter()
            .map(|s| s.as_ref())
            .collect::<Vec<&str>>()
            .join(",");
        let url = format!("me/shows/?ids={}", joined_ids);
        match self.put(&url, &json!({})).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Get a list of shows saved in the current Spotify user’s library. Optional parameters can be used to limit the number of shows returned.
    /// [Get user's saved shows](https://developer.spotify.com/documentation/web-api/reference/library/get-users-saved-shows/)
    /// - limit(Optional). The maximum number of shows to return. Default: 20. Minimum: 1. Maximum: 50
    /// - offset(Optional). The index of the first show to return. Default: 0 (the first object). Use with limit to get the next set of shows.
    pub async fn get_saved_show<L: Into<Option<u32>>, O: Into<Option<u32>>>(
        &self,
        limit: L,
        offset: O,
    ) -> ClientResult<Page<Show>> {
        let mut params = HashMap::new();
        let limit = limit.into().unwrap_or(20);
        let offset = offset.into().unwrap_or(0);
        params.insert("limit".to_owned(), limit.to_string());
        params.insert("offset".to_owned(), offset.to_string());
        let url = "me/shows";
        let result = self.get(url, &mut params).await?;
        self.convert_result::<Page<Show>>(&result)
    }

    /// Get Spotify catalog information for a single show identified by its unique Spotify ID.
    /// [Get a show](https://developer.spotify.com/documentation/web-api/reference/shows/get-a-show/)
    /// Path Parameters:
    /// - id: The Spotify ID for the show.
    /// Query Parameters
    /// - market(Optional): An ISO 3166-1 alpha-2 country code.
    pub async fn get_a_show(&self, id: String, market: Option<Country>) -> ClientResult<FullShow> {
        let url = format!("shows/{}", id);
        let mut params = HashMap::new();
        if let Some(_market) = market {
            params.insert("country".to_owned(), _market.as_str().to_owned());
        }
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<FullShow>(&result)
    }

    /// Get Spotify catalog information for multiple shows based on their Spotify IDs.
    /// [Get seversal shows](https://developer.spotify.com/documentation/web-api/reference/shows/get-several-shows/)
    /// Query Parameters
    /// - ids(Required) A comma-separated list of the Spotify IDs for the shows. Maximum: 50 IDs.
    /// - market(Optional) An ISO 3166-1 alpha-2 country code.
    pub async fn get_several_shows(
        &self,
        ids: impl IntoIterator<Item = &str>,
        market: Option<Country>,
    ) -> ClientResult<SeversalSimplifiedShows> {
        let joined_ids = ids.into_iter().collect::<Vec<&str>>().join(",");
        let url = "shows";
        let mut params = HashMap::new();
        if let Some(_market) = market {
            params.insert("country".to_owned(), _market.as_str().to_owned());
        }
        params.insert("ids".to_owned(), joined_ids);
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<SeversalSimplifiedShows>(&result)
    }
    /// Get Spotify catalog information about an show’s episodes. Optional parameters can be used to limit the number of episodes returned.
    /// [Get a show's episodes](https://developer.spotify.com/documentation/web-api/reference/shows/get-shows-episodes/)
    /// Path Parameters
    /// - id: The Spotify ID for the show.
    /// Query Parameters
    /// - limit: Optional. The maximum number of episodes to return. Default: 20. Minimum: 1. Maximum: 50.
    /// - offset: Optional. The index of the first episode to return. Default: 0 (the first object). Use with limit to get the next set of episodes.
    /// - market: Optional. An ISO 3166-1 alpha-2 country code.
    pub async fn get_shows_episodes<L: Into<Option<u32>>, O: Into<Option<u32>>>(
        &self,
        id: String,
        limit: L,
        offset: O,
        market: Option<Country>,
    ) -> ClientResult<Page<SimplifiedEpisode>> {
        let url = format!("shows/{}/episodes", id);
        let mut params = HashMap::new();
        let limit = limit.into().unwrap_or(20);
        let offset = offset.into().unwrap_or(0);
        params.insert("limit".to_owned(), limit.to_string());
        params.insert("offset".to_owned(), offset.to_string());
        if let Some(_market) = market {
            params.insert("country".to_owned(), _market.as_str().to_owned());
        }
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<Page<SimplifiedEpisode>>(&result)
    }

    /// Get Spotify catalog information for a single episode identified by its unique Spotify ID.
    /// [Get an Episode](https://developer.spotify.com/documentation/web-api/reference/episodes/get-an-episode/)
    /// Path Parameters
    /// - id: The Spotify ID for the episode.
    ///  Query Parameters
    /// - market: Optional. An ISO 3166-1 alpha-2 country code.
    pub async fn get_an_episode(
        &self,
        id: String,
        market: Option<Country>,
    ) -> ClientResult<FullEpisode> {
        let url = format!("episodes/{}", id);
        let mut params = HashMap::new();
        if let Some(_market) = market {
            params.insert("country".to_owned(), _market.as_str().to_owned());
        }
        let result = self.get(&url, &mut params).await?;
        self.convert_result::<FullEpisode>(&result)
    }

    /// Get Spotify catalog information for multiple episodes based on their Spotify IDs.
    /// [Get seversal episodes](https://developer.spotify.com/documentation/web-api/reference/episodes/get-several-episodes/)
    /// Query Parameters
    /// - ids: Required. A comma-separated list of the Spotify IDs for the episodes. Maximum: 50 IDs.
    /// - market: Optional. An ISO 3166-1 alpha-2 country code.
    pub async fn get_several_episodes(
        &self,
        ids: impl IntoIterator<Item = &String>,
        market: Option<Country>,
    ) -> ClientResult<SeveralEpisodes> {
        let url = "episodes";
        let joined_ids = ids
            .into_iter()
            .map(|s| s.as_ref())
            .collect::<Vec<&str>>()
            .join(",");
        let mut params = HashMap::new();
        if let Some(_market) = market {
            params.insert("country".to_owned(), _market.as_str().to_owned());
        }
        params.insert("ids".to_owned(), joined_ids);
        let result = self.get(url, &mut params).await?;
        self.convert_result::<SeveralEpisodes>(&result)
    }

    /// Check if one or more shows is already saved in the current Spotify user’s library.
    /// [Check users saved shows](https://developer.spotify.com/documentation/web-api/reference/library/check-users-saved-shows/)
    /// Query Parameters
    /// - ids: Required. A comma-separated list of the Spotify IDs for the shows. Maximum: 50 IDs.
    pub async fn check_users_saved_shows(
        &self,
        ids: impl IntoIterator<Item = &String>,
    ) -> ClientResult<Vec<bool>> {
        let url = "me/shows/contains";
        let joined_ids = ids
            .into_iter()
            .map(|s| s.as_ref())
            .collect::<Vec<&str>>()
            .join(",");
        let mut params = HashMap::new();
        params.insert("ids".to_owned(), joined_ids);
        let result = self.get(url, &mut params).await?;
        self.convert_result::<Vec<bool>>(&result)
    }

    /// Delete one or more shows from current Spotify user's library.
    /// Changes to a user's saved shows may not be visible in other Spotify applications immediately.
    /// [Remove user's saved shows](https://developer.spotify.com/documentation/web-api/reference/library/remove-shows-user/)
    /// Query Parameters
    /// - ids: Required. A comma-separated list of Spotify IDs for the shows to be deleted from the user’s library.
    /// - market: Optional. An ISO 3166-1 alpha-2 country code.
    pub async fn remove_users_saved_shows(
        &self,
        ids: impl IntoIterator<Item = &String>,
        market: Option<Country>,
    ) -> ClientResult<()> {
        let joined_ids = ids
            .into_iter()
            .map(|s| s.as_ref())
            .collect::<Vec<&str>>()
            .join(",");
        let url = format!("me/shows?ids={}", joined_ids);
        let mut payload = Map::new();
        if let Some(_market) = market {
            payload.insert(
                "country".to_owned(),
                serde_json::Value::String(_market.as_str().to_owned()),
            );
        }
        match self.delete(&url, &Value::Object(payload)).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub fn convert_result<'a, T: Deserialize<'a>>(&self, input: &'a str) -> ClientResult<T> {
        serde_json::from_str::<T>(input).map_err(Into::into)
    }

    /// Append device ID to API path.
    fn append_device_id(&self, path: &str, device_id: Option<String>) -> String {
        let mut new_path = path.to_string();
        if let Some(_device_id) = device_id {
            if path.contains('?') {
                new_path.push_str(&format!("&device_id={}", _device_id));
            } else {
                new_path.push_str(&format!("?device_id={}", _device_id));
            }
        }
        new_path
    }

    fn get_uri(&self, _type: Type, _id: &str) -> String {
        let mut uri = String::from("spotify:");
        uri.push_str(_type.as_str());
        uri.push(':');
        uri.push_str(&self.get_id(_type, _id));
        uri
    }
    /// Get spotify id by type and id
    fn get_id(&self, _type: Type, id: &str) -> String {
        let mut _id = id.to_owned();
        let fields: Vec<&str> = _id.split(':').collect();
        let len = fields.len();
        if len >= 3 {
            if _type.as_str() != fields[len - 2] {
                error!(
                    "expected id of type {:?} but found type {:?} {:?}",
                    _type,
                    fields[len - 2],
                    _id
                );
            } else {
                return fields[len - 1].to_owned();
            }
        }
        let sfields: Vec<&str> = _id.split('/').collect();
        let len: usize = sfields.len();
        if len >= 3 {
            if _type.as_str() != sfields[len - 2] {
                error!(
                    "expected id of type {:?} but found type {:?} {:?}",
                    _type,
                    sfields[len - 2],
                    _id
                );
            } else {
                return sfields[len - 1].to_owned();
            }
        }
        _id.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_get_id() {
        // Assert artist
        let spotify = Spotify::default().access_token("test-access").build();
        let mut artist_id = String::from("spotify:artist:2WX2uTcsvV5OnS0inACecP");
        let id = spotify.get_id(Type::Artist, &mut artist_id);
        assert_eq!("2WX2uTcsvV5OnS0inACecP", &id);
        // Assert album
        let mut artist_id_a = String::from("spotify/album/2WX2uTcsvV5OnS0inACecP");
        assert_eq!(
            "2WX2uTcsvV5OnS0inACecP",
            &spotify.get_id(Type::Album, &mut artist_id_a)
        );

        // Mismatch type
        let mut artist_id_b = String::from("spotify:album:2WX2uTcsvV5OnS0inACecP");
        assert_eq!(
            "spotify:album:2WX2uTcsvV5OnS0inACecP",
            &spotify.get_id(Type::Artist, &mut artist_id_b)
        );

        // Could not split
        let mut artist_id_c = String::from("spotify-album-2WX2uTcsvV5OnS0inACecP");
        assert_eq!(
            "spotify-album-2WX2uTcsvV5OnS0inACecP",
            &spotify.get_id(Type::Artist, &mut artist_id_c)
        );

        let mut playlist_id = String::from("spotify:playlist:59ZbFPES4DQwEjBpWHzrtC");
        assert_eq!(
            "59ZbFPES4DQwEjBpWHzrtC",
            &spotify.get_id(Type::Playlist, &mut playlist_id)
        );
    }
    #[test]
    fn test_get_uri() {
        let spotify = Spotify::default().access_token("test-access").build();
        let track_id1 = "spotify:track:4iV5W9uYEdYUVa79Axb7Rh";
        let track_id2 = "1301WleyT98MSxVHPZCA6M";
        let uri1 = spotify.get_uri(Type::Track, track_id1);
        let uri2 = spotify.get_uri(Type::Track, track_id2);
        assert_eq!(track_id1, uri1);
        assert_eq!("spotify:track:1301WleyT98MSxVHPZCA6M", &uri2);
    }
}
