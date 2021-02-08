-- frontend/src/Main.elm


module Main exposing (main)

import Browser
import Browser.Navigation as Navigation
import Element exposing (Element)
import Element.Background as Background
import Element.Border as Border
import Element.Events as Events
import Element.Font as Font
import Html exposing (Html)
import Html.Attributes as Attr
import Http
import Json.Decode as D
import Json.Encode as E
import Url exposing (Url)


main : Program Flags Model Msg
main =
    let
        layout document =
            { title = document.title
            , body = [ Element.layout [ Element.padding 10 ] document.body ]
            }
    in
    Browser.application
        { init = init
        , onUrlChange = always NoOp
        , onUrlRequest = always NoOp
        , subscriptions = always Sub.none
        , update = update
        , view = view >> layout
        }


type alias Flags =
    ()


type alias Model =
    { statusResponse : Maybe (Result Http.Error Status)
    , logsResponse : Maybe (Result Http.Error Logs)
    }


type Msg
    = NoOp
    | StatusResponse (Result Http.Error Status)
    | GetLogs String String
    | LogsResponse (Result Http.Error Logs)


type alias Status =
    { filesLen : Int
    , indexKeys : List String
    }


statusDecoder : D.Decoder Status
statusDecoder =
    D.map2 Status
        (D.field "files_len" D.int)
        (D.field "index_keys" <| D.list D.string)


encodeStatus : Status -> E.Value
encodeStatus { filesLen, indexKeys } =
    E.object
        [ ( "files_len", E.int filesLen )
        , ( "index_keys", E.list E.string indexKeys )
        ]


type alias Logs =
    List String


logsDecoder : D.Decoder Logs
logsDecoder =
    D.list D.string


init : Flags -> Url -> Navigation.Key -> ( Model, Cmd Msg )
init flags url key =
    ( Model Nothing Nothing
    , Http.get
        { url = "http://localhost:8000/status"
        , expect = Http.expectJson StatusResponse statusDecoder
        }
    )


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        NoOp ->
            ( model, Cmd.none )

        StatusResponse statusResponse ->
            ( { model | statusResponse = Just statusResponse }, Cmd.none )

        GetLogs key value ->
            ( { model | logsResponse = Nothing }
            , Http.get
                { url = "http://localhost:8000/logs/" ++ key ++ "/" ++ value
                , expect = Http.expectJson LogsResponse logsDecoder
                }
            )

        LogsResponse logsResponse ->
            ( { model | logsResponse = Just logsResponse }, Cmd.none )


view : Model -> { title : String, body : Element Msg }
view model =
    { title = "monitoring-rs"
    , body =
        Element.column [ Element.width Element.fill, Element.spacing 10 ]
            [ viewStatusResponse model.statusResponse
            , viewLogsResponse model.logsResponse
            ]
    }


viewStatusResponse : Maybe (Result Http.Error Status) -> Element Msg
viewStatusResponse statusResponse =
    case statusResponse of
        Just (Ok status) ->
            viewStatus status

        Just (Err _) ->
            Element.text "Something went wrong..."

        Nothing ->
            Element.text "Loading..."


viewStatus : Status -> Element Msg
viewStatus { filesLen, indexKeys } =
    let
        viewIndexKeys =
            List.map viewIndexKeyLink indexKeys

        viewIndexKeyLink indexKey =
            let
                maybeKeyValue =
                    case String.split "=" indexKey of
                        key :: value ->
                            Just ( key, String.join "=" value )

                        _ ->
                            Nothing

                content =
                    Element.el
                        [ Font.family [ Font.monospace ] ]
                        (Element.text indexKey)
            in
            case maybeKeyValue of
                Just ( key, value ) ->
                    Element.link
                        [ Events.onClick <| GetLogs key value
                        , Font.color <| Element.rgb 0 0 1
                        ]
                        { url = ""
                        , label = content
                        }

                Nothing ->
                    content
    in
    Element.column [ Element.width Element.fill, Element.spacing 10 ]
        [ Element.row [ Element.width Element.fill, Element.spacing 20 ]
            [ Element.el
                [ Element.width <| Element.fillPortion 1, Font.bold ]
                (Element.text "# log files")
            , Element.el
                [ Element.width <| Element.fillPortion 4 ]
                (Element.text <| String.fromInt filesLen)
            ]
        , Element.row [ Element.width Element.fill, Element.spacing 20 ]
            [ Element.el
                [ Element.width <| Element.fillPortion 1, Font.bold ]
                (Element.text "Metadata entries")
            , Element.column
                [ Element.width <| Element.fillPortion 4 ]
                viewIndexKeys
            ]
        ]


viewLogsResponse : Maybe (Result Http.Error Logs) -> Element Msg
viewLogsResponse logsResponse =
    case logsResponse of
        Just (Ok logs) ->
            let
                viewLog index log =
                    Element.el
                        ([ Element.paddingXY 0 5, Element.width Element.fill ]
                            ++ logBackgroundAttrs index
                        )
                        (Element.text log)

                logBackgroundAttrs index =
                    if modBy 2 index == 0 then
                        [ Background.color <| Element.rgb 0.9 0.9 0.9 ]

                    else
                        []
            in
            Element.column
                [ Border.widthEach { bottom = 0, left = 0, right = 0, top = 1 }
                , Element.paddingXY 0 10
                , Element.width Element.fill
                , Font.family [ Font.monospace ]
                ]
                (List.indexedMap viewLog logs)

        Just (Err _) ->
            Element.text "Something went wrong..."

        Nothing ->
            Element.text ""
