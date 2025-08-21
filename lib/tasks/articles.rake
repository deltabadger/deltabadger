namespace :articles do
  desc 'Import articles from markdown files'
  task import: :environment do
    articles_path = Rails.root.join('articles')

    unless File.directory?(articles_path)
      puts "Creating articles directory: #{articles_path}"
      FileUtils.mkdir_p(articles_path)
      puts 'Add your markdown files to the articles/ directory'
      next
    end

    markdown_files = Dir.glob(File.join(articles_path, '*.md'))

    if markdown_files.empty?
      puts "No markdown files found in #{articles_path}"
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

      # Set attributes
      article.title = metadata['title'] || slug.humanize
      article.subtitle = metadata['subtitle']
      article.content = article_content
      article.excerpt = metadata['excerpt']
      article.thumbnail = metadata['thumbnail']
      article.x_url = metadata['x_url']
      article.telegram_url = metadata['telegram_url']
      article.paywall_hook = metadata['paywall_hook']

      # Handle author by ID
      if metadata['author_id'].present?
        author = Author.find_by(id: metadata['author_id'].to_i)
        if author
          article.author = author
        else
          puts "Warning: Author with ID #{metadata['author_id']} not found for #{filename}"
        end
      end
      article.published = metadata.fetch('published', true)

      if metadata['published_at'].present?
        article.published_at = Time.parse(metadata['published_at'])
      elsif article.published && article.published_at.blank?
        article.published_at = Time.current
      end

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
    articles = Article.all.order(:slug, :locale)

    puts 'Articles:'
    puts '-' * 80

    if articles.empty?
      puts 'No articles found.'
    else
      articles.each do |article|
        puts "#{article.slug}.#{article.locale}"
        puts "  Title: #{article.title}"
        puts "  Author: #{article.author&.name || 'No author'}"
        puts "  Published: #{article.published? ? 'Yes' : 'No'}"
        puts "  Paywall: #{article.paywalled? ? 'Yes' : 'No'}"
        puts "  X URL: #{article.x_url || 'None'}"
        puts "  Telegram URL: #{article.telegram_url || 'None'}"
        puts
      end
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
      subtitle: "A comprehensive guide for beginners"
      author_id: 1
      excerpt: "Learn how to get started with DeltaBadger's automated trading features"
      thumbnail: "https://deltabadger.com/images/getting-started-thumbnail.jpg"
      published: true
      x_url: "https://twitter.com/deltabadger/status/example"
      telegram_url: "https://t.me/deltabadger/example"
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

    puts 'Example articles created in articles/ directory:'
    puts '- getting-started.en.md'
    puts ''
    puts "Run 'rake articles:import' to import them into the database."
  end

  desc 'Manage authors'
  task :authors, [:action] => :environment do |_task, args|
    action = args[:action] || 'list'

    case action
    when 'list'
      authors = Author.all.order(:id)
      if authors.empty?
        puts 'No authors found.'
      else
        puts 'Authors:'
        puts '-' * 60
        authors.each do |author|
          puts "ID: #{author.id}"
          puts "Name: #{author.name}"
          puts "URL: #{author.url}" if author.url.present?
          puts "Avatar: #{author.avatar}" if author.avatar.present?
          puts "Bio: #{author.bio}" if author.bio.present?
          puts "Articles: #{author.articles.count}"
          puts
        end
      end
    when 'create'
      puts 'Creating a new author...'
      print 'Name (required): '
      name = STDIN.gets.chomp

      if name.blank?
        puts 'Name is required!'
        exit 1
      end

      print 'URL (optional): '
      url = STDIN.gets.chomp
      url = nil if url.blank?

      print 'Avatar URL (optional): '
      avatar = STDIN.gets.chomp
      avatar = nil if avatar.blank?

      print 'Bio (optional): '
      bio = STDIN.gets.chomp
      bio = nil if bio.blank?

      author = Author.create!(
        name: name,
        url: url,
        avatar: avatar,
        bio: bio
      )

      puts "✓ Author created with ID: #{author.id}"
    else
      puts 'Usage:'
      puts '  rake articles:authors[list]   - List all authors'
      puts '  rake articles:authors[create] - Create a new author'
    end
  end

  desc 'Remove articles from database'
  task :remove, %i[slug locale] => :environment do |_task, args|
    slug = args[:slug]
    locale = args[:locale]

    if slug.blank?
      puts 'Usage:'
      puts '  rake articles:remove[article-slug]           - Remove all translations of an article'
      puts '  rake articles:remove[article-slug,en]        - Remove specific locale version'
      puts '  rake articles:remove[article-slug,all]       - Remove all translations (explicit)'
      puts ''
      puts 'Available articles:'
      Article.distinct.pluck(:slug).sort.each { |s| puts "  #{s}" }
      exit 1
    end

    if locale.blank? || locale == 'all'
      # Remove all translations of this article
      articles = Article.where(slug: slug)
      if articles.empty?
        puts "No articles found with slug: #{slug}"
        exit 1
      end

      puts "Found #{articles.count} article(s) with slug '#{slug}':"
      articles.each { |article| puts "  #{article.slug}.#{article.locale} - #{article.title}" }
      puts ''
      print 'Are you sure you want to remove ALL translations? (y/N): '
      confirmation = STDIN.gets.chomp.downcase

      if %w[y yes].include?(confirmation)
        count = articles.count
        articles.destroy_all
        puts "✓ Removed #{count} article(s) with slug '#{slug}'"
      else
        puts 'Operation cancelled.'
      end
    else
      # Remove specific locale version
      article = Article.find_by(slug: slug, locale: locale)
      if article.nil?
        puts "No article found with slug: #{slug} and locale: #{locale}"
        available_locales = Article.where(slug: slug).pluck(:locale)
        puts "Available locales for '#{slug}': #{available_locales.join(', ')}" if available_locales.any?
        exit 1
      end

      puts "Found article: #{article.slug}.#{article.locale} - #{article.title}"
      print 'Are you sure you want to remove this article? (y/N): '
      confirmation = STDIN.gets.chomp.downcase

      if %w[y yes].include?(confirmation)
        article.destroy
        puts "✓ Removed article: #{article.slug}.#{article.locale}"
      else
        puts 'Operation cancelled.'
      end
    end
  end

  private

  def extract_metadata(content)
    # Look for YAML frontmatter at the beginning of the file
    if content.start_with?("---\n")
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
