-- Adiciona coluna de resumo gerado pela LLM para cada página analisada
ALTER TABLE pages ADD COLUMN IF NOT EXISTS llm_summary TEXT;
