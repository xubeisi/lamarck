use camino::Utf8PathBuf;
use clap::Args;
use deepgram::{
    transcription::prerecorded::{
        audio_source::AudioSource,
        options::{Language, Options},
    },
    Deepgram, DeepgramError,
};
use indicatif::{ProgressBar, ProgressStyle};
use miette::Diagnostic;
use thiserror::Error;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::*;
use url::Url;

mod srt;
use srt::*;

#[derive(Args, Debug)]
pub struct Caption {
    /// captions require a deepgram API key
    #[clap(env, long)]
    deepgram_api_key: String,
    /// captions language if not englist
    #[clap(env, short, long)]
    deepgram_lang: String,
    /// A path to an audio file or a URL
    /// https://github.com/deepgram-devs/deepgram-rust-sdk/blob/main/src/transcription/prerecorded/options.rs
    #[clap(short, long, value_parser)]
    input: String,
    /// a filepath to use for the output.
    ///
    /// The filename will be preserved if it
    /// exists
    ///
    /// The file extension will be replaced if it
    /// exists
    #[clap(short, long, value_parser)]
    output_path: Option<Utf8PathBuf>,
    /// output the raw deepgram response
    /// as Rust structs.
    ///
    /// Deepgram doesn't supply Serialize for the
    /// Response type.
    #[clap(
        short,
        long,
        default_value_t = false,
        help_heading = "OUTPUT_TYPE"
    )]
    raw: bool,
    /// output an srt file
    #[clap(
        short,
        long,
        default_value_t = false,
        help_heading = "OUTPUT_TYPE"
    )]
    srt: bool,
    /// output a transcript
    #[clap(
        short,
        long,
        default_value_t = false,
        help_heading = "OUTPUT_TYPE"
    )]
    transcript: bool,
    /// output a markdown file with links to video
    /// timestamps
    #[clap(
        short,
        long,
        default_value_t = false,
        help_heading = "OUTPUT_TYPE"
    )]
    markdown: bool,
}

#[derive(Error, Diagnostic, Debug)]
pub enum CaptionError {
    #[error(transparent)]
    #[diagnostic(code(lamarck::io_error))]
    IoError(#[from] std::io::Error),

    #[error(
        "Failed to parse a URL or a FilePath from input"
    )]
    #[diagnostic(code(lamarck::input_parse_error))]
    InputParseError {
        url_error: url::ParseError,
        file_error: camino::FromPathBufError,
    },
    #[error("Deepgram reported an error")]
    #[diagnostic(code(lamarck::deepgram_error))]
    DeepgramError { error: DeepgramError },

    #[error(
      "The supplied output-dir doesn't exist. Create it if you wish to write files there."
    )]
    #[diagnostic(code(lamarck::output_dir_not_exist))]
    OutputDirNotExistError { output_dir: Utf8PathBuf },

    #[error(
        "Couldn't guess a mime type for the input file, try specifying it."
      )]
    #[diagnostic(code(lamarck::mime_could_not_guess))]
    MimeGuessError { filepath: Utf8PathBuf },

    #[error(
        "Media Type (mime) is not an audio file. Deepgram requires an audio file."
      )]
    #[diagnostic(code(lamarck::mime_not_audio))]
    InvalidMimeType { guess: mime_guess::Mime },
}

impl From<DeepgramError> for CaptionError {
    fn from(dg_error: DeepgramError) -> Self {
        CaptionError::DeepgramError { error: dg_error }
    }
}

pub fn str2langopt(lang: &str) -> Language {
    match lang {
        "zh" => Language::zh,
        "zh_CN" => Language::zh_CN,
        "zh_TW" => Language::zh_TW,
        "nl" => Language::nl,
        "en" => Language::en,
        "en_AU" => Language::en_AU,
        "en_GB" => Language::en_GB,
        "en_IN" => Language::en_IN,
        "en_NZ" => Language::en_NZ,
        "en_US" => Language::en_US,
        "fr" => Language::fr,
        "fr_CA" => Language::fr_CA,
        "de" => Language::de,
        "hi" => Language::hi,
        "hi_Latn" => Language::hi_Latn,
        "id" => Language::id,
        "it" => Language::it,
        "ja" => Language::ja,
        "ko" => Language::ko,
        "pt" => Language::pt,
        "pt_BR" => Language::pt_BR,
        "ru" => Language::ru,
        "es" => Language::es,
        "es_419" => Language::es_419,
        "sv" => Language::sv,
        "tr" => Language::tr,
        "uk" => Language::uk,
        _ => Language::en
    }
}

pub async fn generate_captions(
    options: &Caption,
) -> Result<(), CaptionError> {
    let bar = ProgressBar::new(1);

    bar.set_style(ProgressStyle::default_bar()
    .template("[{elapsed_precise}] {spinner} {pos:>7}/{len:7} {msg}")
    .progress_chars("##-"));
    bar.set_message("generating captions...");
    bar.tick();

    let output_location = options
        .output_path
        .clone()
        .unwrap_or(Utf8PathBuf::from("transcript.srt"));
    let output_dir_exists = match output_location
        .file_name()
    {
        Some(_) => {
            // if we have a file name, then make sure the
            // parent dir exists
            if let Some(parent) = output_location.parent() {
                // TODO: what if we only have a filename and
                // no parent dir
                if parent.as_str() == "" {
                    // if the parent is empty, then the file
                    // is in the current
                    // directory
                    true
                } else {
                    parent.exists()
                }
            } else {
                // if the path terminates in a root (like /)
                // or prefix
                true
            }
        }
        None => output_location.exists(),
    };

    if output_location.file_name().is_some() {}

    if !output_dir_exists {
        return Err(CaptionError::OutputDirNotExistError {
            output_dir: output_location,
        });
    }

    let source = match Url::parse(&options.input) {
        Ok(_) => Ok(AudioSource::from_url(&options.input)),
        Err(url_error) => {
            debug!("url failed to parse {:?}", url_error);
            let filepath =
                Utf8PathBuf::from(&options.input);
            let file = File::open(&filepath).await.unwrap();

            match mime_guess::from_path(&options.input)
                .first()
            {
                Some(guess) => {
                    if guess.type_() != "audio" {
                        Err(CaptionError::InvalidMimeType {
                            guess,
                        })
                    } else {
                        Ok(AudioSource::from_buffer_with_mime_type(
                            file,
                            guess.to_string(),
                        ))
                    }
                }
                None => Err(CaptionError::MimeGuessError {
                    filepath: filepath,
                }),
            }
        }
    }?;

    let dg_client =
        Deepgram::new(&options.deepgram_api_key);

    let deepgram_options = Options::builder()
        .punctuate(true)
        .language(str2langopt(&options.deepgram_lang))
        .utterances(true)
        .build();

    bar.set_message("waiting for deepgram");
    let response = dg_client
        .transcription()
        .prerecorded(source, &deepgram_options)
        .await?;

    bar.set_message("processing deepgram response");

    if options.raw {
        let mut output = output_location.clone();
        output.set_extension("raw");
        let mut raw_response_file =
            File::create(output).await?;
        let contents = format!("{:#?}", response);
        raw_response_file
            .write_all(contents.as_bytes())
            .await?;
    }

    if options.transcript {
        let transcript = &response.results.channels[0]
            .alternatives[0]
            .transcript;

        let mut output = output_location.clone();
        output.set_extension("txt");
        let mut transcript_file =
            File::create(output).await?;
        transcript_file
            .write_all(transcript.as_bytes())
            .await?;
    }

    if options.srt {
        let srts = Srt::from(response);
        for (channel_id, channel) in
            srts.channels.iter().enumerate()
        {
            for (alternative_id, alternative) in
                channel.iter().enumerate()
            {
                let mut output = output_location.clone();
                let file_stem = output.file_stem().unwrap();
                let new_file_stem = format!("{file_stem}-channel-{channel_id}-alternative-{alternative_id}");
                output.set_file_name(new_file_stem);
                output.set_extension("srt");

                let mut srt_file =
                    File::create(output).await?;
                srt_file
                    .write_all(alternative.as_bytes())
                    .await?;
            }
        }
    }

    if options.markdown {
        warn!("markdown output is not yet implemented");
    }

    bar.finish_with_message("created caption files");
    Ok(())
}
