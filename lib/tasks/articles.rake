namespace :articles do
  desc 'Import articles from markdown files'
  task import: :environment do
    articles_path = Rails.root.join('articles')

    unless File.directory?(articles_path)
      puts "Creating articles directory: #{articles_path}"
      FileUtils.mkdir_p(articles_path)
      puts 'Add your markdown files to the articles/ directory'
      puts 'Example filename: my-article.en.md'
      next
    end

    markdown_files = Dir.glob(File.join(articles_path, '*.md'))

    if markdown_files.empty?
      puts "No markdown files found in #{articles_path}"
      puts 'Add your markdown files with the pattern: article-slug.locale.md'
      next
    end

    imported_count = 0
    updated_count = 0

    markdown_files.each do |file_path|
      filename = File.basename(file_path, '.md')
      parts = filename.split('.')

      # Expected format: article-slug.locale.md
      if parts.size < 2
        puts "Skipping #{filename}: Invalid format. Expected: article-slug.locale.md"
        next
      end

      locale = parts.last
      slug = parts[0..-2].join('.')

      unless I18n.available_locales.include?(locale.to_sym)
        puts "Skipping #{filename}: Unsupported locale '#{locale}'"
        next
      end

      content = File.read(file_path)
      metadata, article_content = extract_metadata(content)

      article = Article.find_or_initialize_by(slug: slug, locale: locale)
      was_new_record = article.new_record?

      # Set attributes from metadata and content
      article.title = metadata['title'] || slug.humanize
      article.subtitle = metadata['subtitle']
      article.content = article_content
      article.excerpt = metadata['excerpt']
      article.author_name = metadata['author']
      article.author_email = metadata['author_email']
      article.meta_description = metadata['meta_description']
      article.meta_keywords = metadata['meta_keywords']
      article.published = metadata.fetch('published', true)
      article.paywall_marker = metadata.fetch('paywall_marker', '<!-- PAYWALL -->')

      article.published_at = Time.parse(metadata['published_at']) if metadata['published_at'].present?

      if article.save
        if was_new_record
          imported_count += 1
          puts "✓ Imported: #{filename} (#{locale})"
        else
          updated_count += 1
          puts "✓ Updated: #{filename} (#{locale})"
        end
      else
        puts "✗ Failed to save #{filename}: #{article.errors.full_messages.join(', ')}"
      end
    end

    puts "\nImport complete:"
    puts "  Imported: #{imported_count} articles"
    puts "  Updated: #{updated_count} articles"
  end

  desc 'List all articles'
  task list: :environment do
    articles = Article.all

    if articles.empty?
      puts 'No articles found.'
      next
    end

    puts 'Articles:'
    puts '-' * 80

    articles.group_by(&:slug).each do |slug, article_versions|
      first_article = article_versions.first
      locales = article_versions.map(&:locale).sort

      puts "#{slug}"
      puts "  Title: #{first_article.title}"
      puts "  Locales: #{locales.join(', ')}"
      puts "  Published: #{first_article.published? ? 'Yes' : 'No'}"
      puts "  Paywall: #{first_article.has_paywall? ? 'Yes' : 'No'}"
      puts
    end
  end

  desc 'Create example articles'
  task create_examples: :environment do
    articles_path = Rails.root.join('articles')
    FileUtils.mkdir_p(articles_path)

    # Create example article in English
    en_content = <<~CONTENT
      ---
      title: "Getting Started with DeltaBadger"
      author: "John Doe"
      meta_description: "Learn how to get started with DeltaBadger's automated trading features"
      published: true
      ---

      # Getting Started with DeltaBadger

      Welcome to DeltaBadger! This guide will help you get started with our automated trading platform.

      ## What is DeltaBadger?

      DeltaBadger is a powerful automated trading platform that helps you implement **Dollar Cost Averaging (DCA)** strategies for cryptocurrency investments.

      ## Key Features

      - Automated DCA strategies
      - Multiple exchange support
      - Portfolio rebalancing
      - Real-time analytics

      <!-- PAYWALL -->

      ## Premium Features

      As a premium subscriber, you get access to:

      - Advanced portfolio analytics
      - Custom trading strategies
      - Priority customer support
      - API access for developers

      ### Setting Up Your First Bot

      Here's how to set up your first trading bot:

      1. Connect your exchange API keys
      2. Choose your trading pair
      3. Set your DCA parameters
      4. Start your bot

      **Pro Tip**: Start with small amounts while you learn the platform.
    CONTENT

    File.write(File.join(articles_path, 'getting-started.en.md'), en_content)

    # Create example article in German
    de_content = <<~CONTENT
      ---
      title: "Erste Schritte mit DeltaBadger"
      author: "John Doe"
      meta_description: "Erfahren Sie, wie Sie mit den automatisierten Handelsfunktionen von DeltaBadger beginnen"
      published: true
      ---

      # Erste Schritte mit DeltaBadger

      Willkommen bei DeltaBadger! Diese Anleitung hilft Ihnen beim Einstieg in unsere automatisierte Handelsplattform.

      ## Was ist DeltaBadger?

      DeltaBadger ist eine leistungsstarke automatisierte Handelsplattform, die Ihnen bei der Umsetzung von **Dollar Cost Averaging (DCA)** Strategien für Kryptowährungsinvestitionen hilft.

      ## Hauptmerkmale

      - Automatisierte DCA-Strategien
      - Unterstützung mehrerer Börsen
      - Portfolio-Neugewichtung
      - Echtzeitanalysen

      <!-- PAYWALL -->

      ## Premium-Funktionen

      Als Premium-Abonnent erhalten Sie Zugang zu:

      - Erweiterte Portfolio-Analysen
      - Benutzerdefinierte Handelsstrategien
      - Prioritäts-Kundensupport
      - API-Zugang für Entwickler

      ### Einrichtung Ihres ersten Bots

      So richten Sie Ihren ersten Handelsbot ein:

      1. Verbinden Sie Ihre Börsen-API-Schlüssel
      2. Wählen Sie Ihr Handelspaar
      3. Legen Sie Ihre DCA-Parameter fest
      4. Starten Sie Ihren Bot

      **Profi-Tipp**: Beginnen Sie mit kleinen Beträgen, während Sie die Plattform kennenlernen.
    CONTENT

    File.write(File.join(articles_path, 'getting-started.de.md'), de_content)

    puts 'Example articles created in articles/ directory:'
    puts '- getting-started.en.md'
    puts '- getting-started.de.md'
    puts ''
    puts "Run 'rake articles:import' to import them into the database."
  end

  private

  def extract_metadata(content)
    # Look for YAML frontmatter at the beginning of the file
    if content.start_with?("---\n")
      # Find the second occurrence of \n---\n (closing frontmatter)
      lines = content.lines
      closing_line_index = nil

      # Start from line 1 (after opening ---) and look for closing ---
      (1...lines.length).each do |i|
        if lines[i].strip == '---'
          closing_line_index = i
          break
        end
      end

      if closing_line_index
        # Extract YAML content (lines between the --- markers)
        yaml_lines = lines[1...closing_line_index]
        yaml_content = yaml_lines.join.strip

        # Extract article content (everything after closing ---)
        article_lines = lines[(closing_line_index + 1)..-1]
        article_content = article_lines.join.strip

        begin
          metadata = YAML.safe_load(yaml_content) || {}
        rescue Psych::SyntaxError => e
          puts "Warning: Failed to parse YAML frontmatter: #{e.message}"
          puts "YAML content was: #{yaml_content[0..200]}..."
          metadata = {}
        end

        [metadata, article_content]
      else
        puts 'Warning: Could not find closing --- for YAML frontmatter'
        [{}, content]
      end
    else
      [{}, content]
    end
  end
end
